# Proposal

## Why

Installing Microsoft 365 or Office 2024 on Linux takes many manual steps today: a patched Wine build (Bottles' Soda runner), 32-bit system libraries, a dozen Microsoft redistributables, DLL overrides, registry fixes, a hand-written ODT `Configuration.xml`, and hand-made launchers. Since Soda 11.0-32 (2026-10-17), signing in with a personal account works (bottlesdevs/programs#500), so a one-click installer is now feasible for people who already own an Office license. AffinityOnLinux does the same thing for Affinity.

## What Changes

- New Rust + Slint desktop app **Linux Office Installer Suite**, distributed as one self-installing binary. It runs directly on the user's Linux system and never relies on containers.
- Detects the distro and installs the required 64-bit and 32-bit system packages through the native package manager.
  - The Arch family is fully supported and tested.
  - Fedora and Debian/Ubuntu package maps ship labelled **untested**.
  - Other distros get a list of missing libraries.
- Downloads, verifies (SHA256) and manages Bottles Soda Wine runners in `~/.msoffice/runners/`. `soda-11.0-32-experimental` is the default. Users can install newer runners, switch between them, and remove old ones.
- Builds a Wine prefix at `~/.msoffice/prefix` with the steps from the Bottles Microsoft 365 recipe:
  - fonts
  - Microsoft redistributables, downloaded from Microsoft at install time and never bundled
  - DLL overrides, Windows version, registry fixes
- Downloads the Office Deployment Tool from `https://officecdn.microsoft.com/pr/wsus/setup.exe`.
- A simple form generates `Configuration.xml`:
  - product (Microsoft 365 or 2024 Retail), architecture, channel, apps, languages
  - Advanced: updates (off by default), display level, logging
- Runs `setup.exe /configure` and shows live progress read from the Click-to-Run log.
- Users sign in with their own Microsoft account inside Office. The app ships no license, key or activation workaround.
- After installation, detects the installed Office apps and writes `.desktop` launchers to `~/.local/share/applications`.
  - Icons are extracted from the user's own installed EXEs.
  - Launchers declare MIME types so files open in the matching app.
  - Making them the default handlers is an opt-in.
  - Every file the app writes is recorded so it can be removed cleanly.
- Windows 11 style UI: left navigation pane and settings cards, light and dark themes, no gradients anywhere. Built from a small custom component kit instead of Slint's fluent style, because that style uses gradient borders.
- Original app icon (flat, no gradient, not Microsoft branding), generated with Codex-image.
- The app's own settings live in `~/.config/linux-office-installer-suite/config.toml`. Wine runners, the prefix and the download cache live under `~/.msoffice/`.

Non-goals for this change: AppImage packaging (planned later), volume/LTSC editions (KMS/MAK), Visio/Project, fixing Office rendering bugs inside Wine, and verified Fedora/Ubuntu support.

## Capabilities

### New Capabilities
- `system-dependencies`: distro detection, 32-bit repository check, package maps per distro family, privileged install, and a runtime self-check (Wine starts and runs a 32-bit program).
- `wine-runner-management`: list Soda releases from GitHub; download, verify, extract, activate and remove runners; the active runner can't be removed.
- `wine-prefix-setup`: create and prepare `~/.msoffice/prefix` with fonts, redistributables, DLL overrides, registry fixes and the Click-to-Run service; cache downloads.
- `office-deployment-config`: form model to `Configuration.xml` with validation (product/channel compatibility, architecture, excluded apps, languages, updates), plus a preview.
- `office-installation`: download ODT, run `/configure`, track progress, cancel, post-install fixes, detect installed apps, uninstall Office.
- `desktop-integration`: the `msoffice-launch` wrapper, `.desktop` files, icon extraction, MIME associations, optional default handlers, and a record of written files for clean removal.
- `app-settings`: the app's config file, path layout, self-install of the binary and its own launcher.
- `installer-ui`: Windows 11 style shell and pages (Install, Progress & Log, Office apps, Wine runtimes, Settings), theming, the no-gradient component kit, and the app icon.

### Modified Capabilities
(none, since there are no existing specs)

## Impact

- New Cargo project in this repository. Main dependencies:
  - `slint` / `slint-build`
  - an HTTP client (`reqwest` with rustls, blocking or tokio)
  - `sha2`, `tar` + `xz2`, `serde` + `toml`, `quick-xml`, `directories`
- Runtime tools on the user's system: `cabextract` and `icoutils` (`wrestool`/`icotool`), installed by the dependency step.
- Writes only inside the user's home: `~/.msoffice/`, `~/.config/linux-office-installer-suite/`, `~/.local/share/applications/`, `~/.local/share/icons/hicolor/`, `~/.local/bin/`, and `~/.config/mimeapps.list` (only if the user opts in). System packages change only through the distro package manager, after the user confirms.
- Network: GitHub API and release assets, officecdn.microsoft.com, download.microsoft.com / download.windowsupdate.com, the Bottles redistributable proxy and archive.org (for legacy redistributables), and Selawik / corefonts sources.
- Development and testing happen only in the `office` distrobox (Arch). The shipped app never refers to distrobox.
