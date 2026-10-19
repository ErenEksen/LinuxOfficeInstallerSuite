# Spec Delta

## Purpose

Defines where the app stores its own settings and data, the fixed on-disk layout under the user's home, and how the single release binary installs and removes itself.

## ADDED Requirements

### Requirement: Config file location
The system SHALL store its settings in `$XDG_CONFIG_HOME/linux-office-installer-suite/config.toml`, defaulting to `~/.config/linux-office-installer-suite/config.toml`. The settings include the active runner, the last deployment choices, theme preference, the default-handler option and install state. A missing or unreadable config SHALL be replaced with defaults, without crashing. An unreadable file SHALL be kept as a backup.

#### Scenario: Corrupt config
- **WHEN** `config.toml` contains invalid TOML
- **THEN** the app starts with default settings, renames the bad file to `config.toml.bak`, and shows a notice

#### Scenario: Choices remembered
- **WHEN** the user changes the product and restarts the app
- **THEN** the Install page shows the previously chosen product

### Requirement: Data layout
The system SHALL keep all Wine data under `~/.msoffice/`, in these subdirectories:
- `runners/` (Wine runners)
- `prefix/` (the Wine prefix)
- `cache/` (downloads)
- `bin/` (the launch wrapper)
- `logs/` (operation logs)

It SHALL NOT write Wine data anywhere else.

#### Scenario: Fresh system
- **WHEN** the app runs its first installation
- **THEN** `~/.msoffice/` is created with the listed subdirectories and nothing Wine-related is written outside it

### Requirement: No system-wide changes without consent
Apart from the package installation the user confirms, the system SHALL only write inside the user's home directory. It SHALL NOT require running as root, and SHALL refuse to start when run as root.

#### Scenario: Started as root
- **WHEN** the app is started with UID 0
- **THEN** it shows an error explaining it must run as a normal user, then exits

### Requirement: Self-install
When the binary runs from outside `~/.local/bin/`, the system SHALL offer "Install Linux Office Installer Suite for this user". This copies the binary to `~/.local/bin/linux-office-installer-suite`, installs the app icon, and writes its own desktop entry. All of these are recorded in the ledger.

#### Scenario: First run from Downloads
- **WHEN** the user starts the binary from `~/Downloads`
- **THEN** the app offers self-install, and after accepting, "Linux Office Installer Suite" appears in the application menu

### Requirement: Self-uninstall
Settings SHALL offer "Remove Linux Office Installer Suite". It removes the app's own binary, icon, desktop entry and config directory. It SHALL ask separately whether to also remove `~/.msoffice/` and the Office launchers.

#### Scenario: Remove app but keep Office
- **WHEN** the user removes the app and declines removing Office data
- **THEN** the app's own files are deleted while `~/.msoffice/` and Office launchers remain usable

### Requirement: Remove everything
Settings SHALL offer "Remove everything", behind a confirmation dialog that lists what is deleted and warns that files inside the Wine C: drive go too. It SHALL close the prefix's Wine processes, then delete `~/.msoffice/` (Office, prefix, runners, downloads, logs), the Office launchers, icons and file associations (restoring the previous defaults), and the app's own binary, desktop entry, icon and config directory. Afterwards it SHALL also remove files with the app's exact names (`msoffice-<app>.desktop`, `msoffice-<app>` and `linux-office-installer-suite` icons, the `~/.local/bin` copy, and its `msoffice-*` associations in `mimeapps.list`) even when the ledger no longer lists them. Other applications' files SHALL NOT be touched. The work SHALL run off the UI thread, and the app SHALL quit when it finishes.

#### Scenario: Nothing is left behind
- **WHEN** the user confirms Remove everything after a complete Office install
- **THEN** `~/.msoffice/`, the app's config directory, every `msoffice-*` desktop entry and icon, the app's own entry, icon and binary are gone, `mimeapps.list` no longer names any `msoffice-*` entry, and the app exits

#### Scenario: Lost ledger
- **WHEN** the ledger is missing but Office launchers and icons still exist
- **THEN** Remove everything still deletes them by name and leaves other applications' entries and icons in place

### Requirement: Operation logs
Every long-running operation (dependency install, runner install, prefix setup, Office setup) SHALL write a timestamped log file into `~/.msoffice/logs/`. Settings SHALL offer "Open logs folder".

#### Scenario: Log after failure
- **WHEN** prefix setup fails
- **THEN** a log file for that run exists in `~/.msoffice/logs/` and the error view links to it
