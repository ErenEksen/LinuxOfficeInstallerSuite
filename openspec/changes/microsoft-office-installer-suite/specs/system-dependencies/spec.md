# Spec Delta

## Purpose

Makes sure the user's Linux system has every 64-bit and 32-bit library and tool that the Soda Wine runner and the installer need. Missing ones are installed through the distro's own package manager, or listed for the user to install by hand.

## ADDED Requirements

### Requirement: Distro family detection
The system SHALL determine the distro family by reading `ID` and `ID_LIKE` from `/etc/os-release`. It SHALL classify the system as one of `arch`, `fedora`, `debian`, or `unknown`.

#### Scenario: Arch derivative detected
- **WHEN** `/etc/os-release` contains `ID=cachyos` and `ID_LIKE=arch`
- **THEN** the system classifies the distro family as `arch`

#### Scenario: Ubuntu detected through ID_LIKE
- **WHEN** `/etc/os-release` contains `ID=linuxmint` and `ID_LIKE="ubuntu debian"`
- **THEN** the system classifies the distro family as `debian`

#### Scenario: Unrecognised distro
- **WHEN** neither `ID` nor `ID_LIKE` matches a supported family
- **THEN** the system classifies the distro family as `unknown`

### Requirement: Support level labelling
The system SHALL show a support level for the detected family: `arch` is "Supported". `fedora` and `debian` are "Untested". `unknown` is "Manual". For untested families the UI SHALL state that automatic package installation has not been verified.

#### Scenario: Fedora shows untested label
- **WHEN** the detected family is `fedora`
- **THEN** the dependency page shows the support level "Untested" with an explanation

### Requirement: Required library check
The system SHALL check for every required 64-bit shared library and every required 32-bit shared library that the runner needs, plus the helper tools `cabextract`, `wrestool` and `icotool`. It SHALL report each missing item by name. Required 64-bit libraries SHALL include `libunwind.so.8`. The required 32-bit set SHALL include the 32-bit dynamic loader (`ld-linux.so.2`).

#### Scenario: All present
- **WHEN** every required library and tool is found
- **THEN** the dependency check reports "ready" and the install flow may continue

#### Scenario: 32-bit loader missing
- **WHEN** `ld-linux.so.2` is not present on the system
- **THEN** the dependency check reports the 32-bit libraries as missing and the install flow is blocked until they are installed

### Requirement: 32-bit repository check
Before installing packages, the system SHALL check that the distro's 32-bit package source is enabled: `[multilib]` in `/etc/pacman.conf` on `arch`, and the `i386` foreign architecture on `debian`. If it is not enabled, the system SHALL NOT edit system configuration itself. It SHALL show the user the exact steps to enable it.

#### Scenario: Multilib disabled on Arch
- **WHEN** the family is `arch` and `/etc/pacman.conf` has no active `[multilib]` section
- **THEN** the system shows instructions to enable multilib and does not start package installation

### Requirement: Privileged package installation
For `arch`, `fedora` and `debian`, the system SHALL install all missing packages in one privileged package-manager command, after the user confirms. It SHALL show the exact command before running it. The system SHALL NOT request elevated privileges for any other operation.

#### Scenario: User confirms install
- **WHEN** packages are missing on a supported or untested family and the user clicks "Install packages"
- **THEN** the system shows the package list and command, and asks for authentication. It runs the command and then re-runs the dependency check

#### Scenario: Install fails
- **WHEN** the package-manager command exits non-zero or authentication is cancelled
- **THEN** the system shows the error output and keeps the install flow blocked

### Requirement: Manual fallback
For the `unknown` family, the system SHALL list the missing library and tool names. It SHALL offer a "Check again" action instead of installing anything.

#### Scenario: Unknown distro missing libraries
- **WHEN** the family is `unknown` and libraries are missing
- **THEN** the system lists the missing names and offers "Check again", without running any package manager

### Requirement: Runtime self-check
After dependencies are satisfied and a runner is active, the system SHALL check that Wine works. It SHALL run the runner's version command, and run a 32-bit Windows program in a scratch prefix. Office installation SHALL only become available after both succeed.

#### Scenario: 32-bit execution fails
- **WHEN** the 32-bit test program fails to start
- **THEN** the system reports the self-check failure with the captured error output and blocks Office installation
