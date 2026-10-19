# Spec Delta

## Purpose

Lets the user install, switch between and remove Bottles Soda Wine runners in `~/.msoffice/runners/`, so Office can move to newer Wine builds as they are released.

## ADDED Requirements

### Requirement: Default runner
The system SHALL use `soda-11.0-32-experimental` (x86_64) as the default runner. It SHALL install the default runner automatically during the first Office installation if no runner is installed.

#### Scenario: First install without runner
- **WHEN** the user starts an Office installation and no runner is installed
- **THEN** the system installs `soda-11.0-32-experimental` and makes it active before continuing

### Requirement: List available runners
The system SHALL list Soda releases from the `bottlesdevs/wine` GitHub releases whose tag starts with `soda-` and that have an `x86_64` archive asset, newest first. It SHALL mark prereleases and runners that are already installed.

#### Scenario: Listing releases
- **WHEN** the user opens the Wine runtimes page and selects "Install new version"
- **THEN** the system shows available Soda releases with tag, date, prerelease flag and installed state

#### Scenario: Offline
- **WHEN** the GitHub API cannot be reached
- **THEN** the system shows an error and still lists installed runners

### Requirement: Verified runner download
The system SHALL download the runner's `x86_64` archive and its `SHA256SUMS` file. It SHALL verify the archive hash before extracting. It SHALL extract into `~/.msoffice/runners/<tag>/`. A failed download or a hash mismatch SHALL leave no partial runner directory behind.

#### Scenario: Hash mismatch
- **WHEN** the downloaded archive's SHA-256 differs from the entry in `SHA256SUMS`
- **THEN** the system deletes the download, reports the mismatch, and does not install the runner

#### Scenario: Successful install
- **WHEN** download and verification succeed
- **THEN** the runner appears as installed in the Wine runtimes list, with download progress shown during the operation

### Requirement: Active runner selection
Exactly one installed runner SHALL be active at a time. The choice SHALL be stored in the app config. Switching the active runner SHALL take effect on the next Office launch without regenerating launchers.

#### Scenario: Switch runner
- **WHEN** the user selects "Set as active" on an installed runner
- **THEN** the config records the new active runner and later Office launches use it

### Requirement: Runner removal
The system SHALL let the user remove any installed runner except the active one, after a confirmation dialog. Removal SHALL delete the runner's directory.

#### Scenario: Remove inactive runner
- **WHEN** the user confirms removing an inactive runner
- **THEN** its directory under `~/.msoffice/runners/` is deleted and it disappears from the installed list

#### Scenario: Attempt to remove active runner
- **WHEN** the user opens the menu of the active runner
- **THEN** the remove action is disabled with the explanation that the active runner cannot be removed
