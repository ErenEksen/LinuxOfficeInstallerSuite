# Spec Delta

## Purpose

Runs the end-to-end Office installation from one Install click: download the official Office Deployment Tool, run it with the generated configuration, report progress, and apply the post-install fixes Office needs under Soda.

## ADDED Requirements

### Requirement: One-click install pipeline
Clicking "Install" SHALL run these stages in order:
1. dependency check
2. runner install, if needed
3. runtime self-check
4. prefix setup
5. ODT download
6. Office files download (local Office source)
7. Click-to-Run service registration
8. Office setup
9. post-install fixes
10. app detection and launcher creation

Each stage SHALL be shown with its status: pending, running, done, or failed. A failure SHALL stop the pipeline and show the stage's error with its log.

#### Scenario: Happy path
- **WHEN** the user clicks Install on a system with all dependencies present
- **THEN** all stages run in order and the Progress page ends with a success banner and links to launch the installed apps

#### Scenario: Stage failure
- **WHEN** the Office setup stage exits with a non-zero code
- **THEN** the pipeline stops, later stages stay pending, and the page shows the exit code and the path of the setup log

### Requirement: Official ODT only
The system SHALL download the Office Deployment Tool only from `https://officecdn.microsoft.com/pr/wsus/setup.exe` at install time. It SHALL check that the file is a Windows PE executable before running it.

#### Scenario: Invalid download
- **WHEN** the downloaded file is not a PE executable
- **THEN** the system deletes it and reports the ODT download as failed

### Requirement: Local Office source
Before Office setup runs, the system SHALL download the Office installation files natively from `https://officecdn.microsoft.com/pr/<channel GUID>/Office/Data/` into `~/.msoffice/cache/office-source/<channel>/`, in the layout `setup.exe /download` produces. The build SHALL be the channel's default `AvailableBuild` from `v64.cab`/`v32.cab`. Files of 32 MiB or more SHALL be downloaded over parallel `Range` connections and SHALL resume with only the missing chunks after an interruption or cancel. Setup SHALL then install from that folder: the installed `Configuration.xml` is the preview plus `SourcePath` (through the prefix's `Z:` drive), `Version` and `AllowCdnFallback="TRUE"` on `<Add>`. Only the current build per channel SHALL be kept in the cache.

#### Scenario: Reinstall from the cache
- **WHEN** the files for the channel's current build are already in the cache
- **THEN** the stage downloads nothing and setup installs from the cache

#### Scenario: Interrupted download
- **WHEN** the download stops because of a network error or Cancel
- **THEN** the stage fails or is cancelled, the finished chunks stay, and the next install downloads only what is missing

#### Scenario: CDN layout changed
- **WHEN** the build can't be read or a required file is missing on the CDN
- **THEN** the stage logs why, and setup runs without `SourcePath` and downloads the files itself

### Requirement: Live installation progress
While Office setup runs, the system SHALL show a percentage progress bar. The percentage comes from the Click-to-Run log lines `total progress is now <N>` in the prefix's temp folder (UTF-16LE). If no percentage is available yet, the bar SHALL be indeterminate. The raw log SHALL be viewable in a collapsible log panel.

#### Scenario: Progress updates
- **WHEN** the Click-to-Run log reports `total progress is now 42`
- **THEN** the progress bar shows 42%

### Requirement: Cancel installation
The user SHALL be able to cancel a running installation after confirming. Cancelling SHALL stop all Wine processes of the prefix and mark the pipeline as cancelled. Completed earlier stages SHALL be kept.

#### Scenario: Cancel during setup
- **WHEN** the user confirms Cancel while Office setup is running
- **THEN** the prefix's Wine processes are terminated and the Install button becomes available again

### Requirement: Click-to-Run service registration
Before running Office setup, the system SHALL register the `ClickToRunSvc` Windows service in the prefix, pointing at `OfficeClickToRun.exe /service`. If the service already exists, that SHALL be treated as success.

#### Scenario: Service already registered
- **WHEN** setup is re-run on a prefix where `ClickToRunSvc` exists
- **THEN** registration reports success and setup continues

### Requirement: Post-install fixes
After setup succeeds, the system SHALL:
- copy `AppvIsvSubsystems64.dll` and `C2R64.dll` from the ClickToRun folder into the `Office16` folder
- set `sppc` to `builtin,native`
- set `HKCU\Software\Microsoft\Office\16.0\Common\Licensing\LicensingNext\<ProductID>` to `2`

#### Scenario: Fixes applied
- **WHEN** Office setup completes successfully
- **THEN** both DLLs exist in `Office16` and the LicensingNext value for the chosen product is 2

### Requirement: Account-based activation only
The system SHALL leave activation to the user signing in with their own Microsoft account inside an Office app. The system SHALL NOT ship, generate or request product keys, and SHALL NOT modify licensing beyond the values listed in this spec.

#### Scenario: First launch after install
- **WHEN** installation finishes
- **THEN** the success banner tells the user to open an Office app and sign in with their Microsoft account

### Requirement: Installed app detection
After installation, and whenever the Office apps page opens, the system SHALL detect which of WINWORD, EXCEL, POWERPNT, OUTLOOK, ONENOTE, MSACCESS and MSPUB exist under `Program Files/Microsoft Office/root/Office16/`. It SHALL list only those as installed.

#### Scenario: Partial install
- **WHEN** Access and Publisher were excluded
- **THEN** the Office apps page lists Word, Excel, PowerPoint, Outlook and OneNote only

### Requirement: Uninstall Office
The system SHALL offer "Uninstall Office". After confirmation, it removes all launchers and associations the app created. It then either deletes the prefix (default) or keeps it, as the user chooses.

#### Scenario: Full uninstall
- **WHEN** the user confirms Uninstall Office with "Delete prefix" selected
- **THEN** all recorded launcher, icon and association files are removed and `~/.msoffice/prefix` is deleted
