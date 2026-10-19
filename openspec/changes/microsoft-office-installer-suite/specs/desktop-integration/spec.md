# Spec Delta

## Purpose

Makes each installed Office app a normal Linux desktop application: a menu entry with its own icon, launchable with files, and able to open the matching document types. Everything is written only inside the user's home and is fully removable.

## ADDED Requirements

### Requirement: Launch wrapper
The system SHALL install a launch wrapper at `~/.msoffice/bin/msoffice-launch`. Given an app identifier and optional file paths, the wrapper SHALL:
- run the app with the currently active runner and the `~/.msoffice/prefix` prefix
- convert each Linux path to a Windows path
- reset the permissions of `ProgramData/Microsoft/Office/ClickToRunPackageLocker` to `0770` before launching

#### Scenario: Open a document
- **WHEN** the user runs `msoffice-launch word /home/u/Documents/report.docx`
- **THEN** Word starts in the prefix with the active runner and opens `report.docx`

#### Scenario: Runner changed
- **WHEN** the active runner is switched after launchers were created
- **THEN** the next launch through the wrapper uses the new runner without any launcher being rewritten

### Requirement: Desktop entries
For each installed Office app, the system SHALL write `~/.local/share/applications/msoffice-<app>.desktop`. Each entry SHALL have:
- the app's name
- an `Exec` line calling the wrapper with a file field code
- the icon
- `Categories=Office`
- a `MimeType` list covering the app's document formats

The system SHALL refresh the desktop database when the tool is available.

#### Scenario: Word entry
- **WHEN** Word is detected as installed
- **THEN** `msoffice-word.desktop` exists, appears in the menu as "Microsoft Word", and declares MIME types including `.docx`, `.doc`, `.rtf` and `.odt`

### Requirement: MIME type coverage
The launchers SHALL declare these MIME types:
- Word: docx, docm, dotx, dotm, doc, dot, rtf, odt
- Excel: xlsx, xlsm, xltx, xltm, xlsb, xls, xlt, csv, ods
- PowerPoint: pptx, pptm, potx, potm, ppsx, ppsm, ppt, pot, pps, odp
- Outlook: msg, eml
- OneNote: one
- Access: accdb, mdb
- Publisher: pub

#### Scenario: Spreadsheet association
- **WHEN** the user right-clicks an `.xlsx` file in a file manager after installation
- **THEN** "Microsoft Excel" is offered as an application to open it

### Requirement: Icons from the user's installation
The system SHALL extract each app's icon from its installed executable on the user's machine. It SHALL install the icon into `~/.local/share/icons/hicolor/<size>/apps/` under a unique `msoffice-<app>` name. If extraction fails, the system SHALL use a generic built-in document icon. Microsoft icons SHALL NOT be shipped with the application.

#### Scenario: Extraction fails
- **WHEN** no icon can be extracted from `MSPUB.EXE`
- **THEN** the Publisher launcher uses the built-in generic icon and installation still succeeds

### Requirement: Optional default handlers
The system SHALL offer a "Set as default for Office files" option, off by default. When it is enabled, the system SHALL make each installed app the default handler for its MIME types in the user's `mimeapps.list`. It SHALL record the previous defaults so they can be restored.

#### Scenario: Opt-in disabled
- **WHEN** the user installs Office without enabling the option
- **THEN** the user's `mimeapps.list` is not modified

#### Scenario: Restore on removal
- **WHEN** the option was enabled and the user uninstalls Office
- **THEN** the recorded previous default handlers are restored

### Requirement: Recorded file ledger
The system SHALL record every file it writes outside `~/.msoffice/` in a ledger stored in the app config directory: desktop entries, icons, the app's own launcher and binary, and MIME default changes. Removal operations SHALL delete exactly the ledger entries and nothing else.

#### Scenario: Clean removal
- **WHEN** the user removes launchers
- **THEN** every file listed in the ledger is deleted and no other file in the applications or icons directories is touched
