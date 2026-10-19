# Spec Delta

## Purpose

Prepares the Wine prefix at `~/.msoffice/prefix` with the fonts, Microsoft redistributables, DLL overrides and registry settings that Office Click-to-Run needs under Soda, without bundling any Microsoft binary.

## ADDED Requirements

### Requirement: Prefix location and architecture
The system SHALL create a 64-bit Wine prefix at `~/.msoffice/prefix` using the active runner. Wine's menu builder SHALL be disabled for every process the system starts in this prefix. That keeps Wine from creating its own launchers or file associations.

#### Scenario: Fresh prefix
- **WHEN** prefix setup runs and `~/.msoffice/prefix` does not exist
- **THEN** a 64-bit prefix is created there and no Wine-generated `.desktop` files appear in the user's applications directory

### Requirement: Redistributables downloaded at install time
The system SHALL obtain every Microsoft component only by downloading it from its original or archived public source during setup. Those components are `gdiplus`, `sppc`, `t2embed` (from Windows 7 SP1 KB976932), `msxml6`, `riched20`, `msls31`, `cmd.exe` (Q811493) and the core fonts. Each download SHALL be checked against a known checksum and cached in `~/.msoffice/cache/`. The application package SHALL contain no Microsoft binaries.

#### Scenario: Cached redistributable reused
- **WHEN** a redistributable archive with a matching checksum already exists in the cache
- **THEN** the system uses it without downloading it again

#### Scenario: Checksum mismatch
- **WHEN** a downloaded redistributable does not match its expected checksum
- **THEN** the system deletes it, reports which component failed, and stops prefix setup

### Requirement: Prefix configuration matches the Bottles Microsoft 365 recipe
The system SHALL apply this configuration to the prefix:
- Windows version: win7 while installing components, then win10.
- Fonts: core fonts and Selawik, with Segoe UI font substitutes.
- DLL overrides: `cmd.exe`, `msxml6`, `riched20`, `t2embed`, `msls31` = native,builtin. `gdiplus` = native. `sppc` and `winscard` = builtin.
- Direct3D `MaxVersionGL` = `0x30002`, with Direct2D `max_version_factory` removed.
- The OpenGL renderer and the Wayland graphics driver enabled.
- DXVK installed.

#### Scenario: Overrides applied
- **WHEN** prefix setup completes
- **THEN** the prefix registry contains the listed DLL overrides and the Windows version reports Windows 10

### Requirement: Resumable and idempotent setup
Prefix setup SHALL record each completed step. If it is interrupted, it SHALL resume from the first incomplete step. Re-running setup on a completed prefix SHALL make no changes.

#### Scenario: Interrupted setup
- **WHEN** setup was interrupted after the fonts step
- **THEN** the next run skips the completed steps and continues with the redistributables

### Requirement: Prefix reset
The system SHALL offer a "Reset prefix" action. After confirmation, it deletes `~/.msoffice/prefix` and the generated launchers. It keeps runners and the download cache.

#### Scenario: Reset
- **WHEN** the user confirms "Reset prefix"
- **THEN** the prefix and the app's launchers are removed, while `~/.msoffice/runners/` and `~/.msoffice/cache/` remain
