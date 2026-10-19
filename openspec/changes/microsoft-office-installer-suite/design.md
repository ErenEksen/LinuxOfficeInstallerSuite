# Design

## Context

This is a greenfield repository: only `openspec/` exists. See proposal.md for the motivation. The facts that shape the approach were checked during exploration on 2026-10-19 in the `office` distrobox (Arch). That container is the only build and test environment, and the shipped app never refers to it.

- **Runner layout:** `soda-11.0-32-experimental-x86_64.tar.xz` (223 MB, 1017 MB unpacked) contains `bin/wine` (64-bit), `lib/wine/x86_64-unix` (308 libs), and `lib/wine/i386-unix` (306 libs plus a 32-bit `wine` and `wine-preloader`). It is **old-style shared WoW64**. Without the host's 32-bit glibc, `wineboot` fails with `/lib/ld-linux.so.2: could not open`. With the 64-bit and 32-bit library sets installed, the prefix builds and both 32-bit and 64-bit `reg.exe` run (`ntsync: up and running`).
- **libunwind:** the 64-bit `ntdll.so` links `libunwind.so.8` directly, so it is required.
- **ODT:** `https://officecdn.microsoft.com/pr/wsus/setup.exe` is the ODT itself: PE32 i386, v16.0.20326.20112, about 7 MB.
- **Reference recipe:** the working steps come from Bottles: `bottlesdevs/programs/Software/microsoft365*.yml` and its `office-runtime`, `gdiplus`, `msxml6`, `riched20`, `msls31` and `selawik` dependencies. Download URLs and MD5 checksums are taken from those files.
- **Multilib:** the Arch container image ships with no `[multilib]` section, so the 32-bit repository can't be assumed even on Arch.
- **Slint styling:** Slint 1.18.1's `fluent` style paints Button, LineEdit and ComboBox borders with `@linear-gradient`.

### Handoff notes for implementation (state as of 2026-10-19)

**Working rules from the user:**
- Build, install and test only inside the `office` distrobox: `distrobox enter office -- <cmd>`, HOME=`/home/axis/distrobox/office`, Arch, passwordless `sudo`.
- Never write to the real host home or system.
- The shipped app must never call or mention distrobox.
- Subagents are allowed where useful. Images are made with the Codex-image MCP tool (`mcp__codex-image__generate_image`).
- No gradients anywhere in the UI.

**Current container state:**
- `[multilib]` (Include = /etc/pacman.d/mirrorlist) was appended to `/etc/pacman.conf`.
- Installed and tested, these let Soda start and run both 32-bit and 64-bit programs:
  - 64-bit: `libunwind freetype2 fontconfig gnutls vulkan-icd-loader libglvnd mesa libx11 libxrandr libxi libxcursor libxcomposite libxinerama libxrender libxfixes libxxf86vm dbus krb5 libcups libpulse alsa-lib wayland libxkbcommon systemd-libs libusb pcsclite cabextract icoutils`
  - 32-bit: `lib32-glibc lib32-freetype2 lib32-fontconfig lib32-gnutls lib32-vulkan-icd-loader lib32-libglvnd lib32-mesa lib32-libx11 lib32-libxrandr lib32-libxcursor lib32-libxrender lib32-libxxf86vm lib32-libxext lib32-libpulse lib32-alsa-lib lib32-wayland lib32-libxkbcommon lib32-systemd lib32-dbus lib32-libcups lib32-krb5 lib32-gmp lib32-libgcrypt`
- This list is the starting point for the Arch map in `deps/maps.rs`. Rust and cargo are **not** installed yet (task 1.1).
- `~/spike/` holds the exploration leftovers, which can be reused to avoid re-downloading:
  - `soda/soda-11.0-32-experimental-x86_64/` (extracted, checksum verified) and its `.tar.xz`
  - `prefix/` (a test prefix created with that runner)
  - `odt/setup.exe` (officecdn)

  Task 12.1 deletes it.

**Reference sources (fetch raw files when porting):**
- Office recipe: `https://raw.githubusercontent.com/bottlesdevs/programs/main/Software/microsoft365enterprise.yml` (business variant: `microsoft365business.yml`). Steps, registry values, the progress regex and the `sc create` arguments are here.
- Dependency recipes (URLs, MD5, sizes, cab member paths): `https://raw.githubusercontent.com/bottlesdevs/dependencies/main/Essentials/{office-runtime,gdiplus,msxml6,riched20,msls31}.yml` and `.../Fonts/{selawik,andale32,arial32,arialb32,comic32,courie32,georgi32,impact32,times32,trebuc32,verdan32,webdin32}.yml`.
- Runner releases: `https://api.github.com/repos/bottlesdevs/wine/releases`. Assets: `<tag>-x86_64.tar.xz` and `SHA256SUMS`. Ignore the `-symbols` and `aarch64` assets.
- Known Office-under-Soda problems and status: `https://github.com/bottlesdevs/programs/issues/500`.
- ODT element reference: `https://learn.microsoft.com/en-us/microsoft-365-apps/deploy/office-deployment-tool-configuration-options`. Product IDs: `https://learn.microsoft.com/en-us/troubleshoot/microsoft-365-apps/office-suite-issues/product-ids-supported-office-deployment-click-to-run`.
- Slint 1.18.1 (latest as of 2026-09-21). The gradient brushes are in `internal/compiler/widgets/fluent/styling.slint`.

**Windows 11 measurements for the UI kit** (from WinUI / Community Toolkit sources; the spec gives colours, radii and the type ramp):
- Nav pane: 320 px open, 48 px compact; collapse below 1008 px.
- SettingsCard: padding 16, min height 68, icon-to-text gap 20, header-to-description gap 8, description in Caption (12).
- Page content: max width about 1000 px, section headers in Body Strong.
- ToggleSwitch track 40×20 with a 12 px thumb. ProgressBar 3 px high with 3 px radius.
- Dark card fill about `#2B2B2B`, stroke about `#1D1D1D`. Light card `#FDFDFD`, stroke `#E5E5E5`. Secondary text `#5C5C5C` light, `#CCCCCC` dark.

## Goals / Non-Goals

**Goals:**
- One binary, no runtime services, and no root except the confirmed package-manager call.
- Every step can resume safely: a crashed or cancelled run continues where it stopped.
- Office keeps working if the installer app is removed. Launchers depend only on files in `~/.msoffice/`.
- The pure logic (XML generation, os-release and ldconfig parsing, ledger, checksums) is unit-testable without Wine.

**Non-Goals:**
- No AppImage packaging, though nothing here should block it later.
- No general Wine prefix manager. There is exactly one prefix.
- No Office rendering fixes beyond the Bottles recipe values.
- No automatic editing of `/etc/pacman.conf` or apt sources.

## Decisions

### D1. Single crate, layered modules
```
src/
  main.rs            root check, single-instance, start UI
  core/
    paths.rs         ~/.msoffice/{runners,prefix,cache,bin,logs}, XDG dirs
    config.rs        config.toml (serde), defaults, corrupt-file backup
    ledger.rs        record of files written outside ~/.msoffice
    download.rs      HTTP GET with resume (.part + Range), progress callback, sha256/md5
    proc.rs          spawn with env, stream stdout/stderr lines, cancel token
  deps/              os_release.rs, ldconfig.rs, maps.rs (soname -> pkg per family), install.rs
  runner/            github.rs (release list), install.rs, active.rs
  prefix/            steps.rs (step list + state file), redist.rs (recipe table), wine.rs (reg/override helpers)
  office/            config_xml.rs, odt.rs, pipeline.rs, progress.rs (C2R log tail), detect.rs
  desktop/           launcher.rs (wrapper script), entries.rs, icons.rs, mime.rs
  ui/                bridge.rs (Rust <-> Slint models/callbacks)
ui/                  *.slint (theme.slint, kit/*.slint, pages/*.slint, app.slint)
assets/              icon PNGs, Inter font
```
**Alternative:** a cargo workspace with core and ui crates. Not worth it at this size. The `core` modules don't depend on Slint, which keeps them testable.

### D2. Threads plus `invoke_from_event_loop`, not async
Slint owns the main thread. Long operations run on one worker thread with a blocking `reqwest` client (rustls). Workers send UI updates with `slint::invoke_from_event_loop`. Cancellation is an `Arc<AtomicBool>` checked between chunks and steps. For Wine, cancelling also runs `wineserver -k` for the prefix.
**Alternative:** tokio. It adds complexity without benefit: work is mostly sequential, with one download or one process at a time.

### D3. Pipeline of resumable steps
Prefix setup and installation are an ordered list of `Step { id, title, run(ctx) }`. Finished step ids are stored in `~/.msoffice/prefix/.lois-state.toml`. That file is tied to the prefix, so deleting the prefix resets it. Re-running skips finished steps (wine-prefix-setup: Resumable and idempotent setup). The Progress page is rendered from this same list.

### D4. Redistributables as an embedded recipe table, extracted with `cabextract`
The Bottles YAML steps are ported to a static Rust table: `{ name, url, md5, size, extract: [(member path, dest dir, rename)], overrides }`. Each archive is downloaded once into `~/.msoffice/cache/` (the 947 MB Win7 SP1 package is shared by gdiplus, sppc and t2embed) and extracted with the `cabextract` CLI. `cabextract` also finds the cabinets embedded in `.exe` and `.msi` files, which is how Bottles' `get_from_cab` works. During implementation the SHA-256 of each archive is computed once and pinned next to the MD5, and both are checked.
Core fonts come from the same SourceForge `corefonts` archives that winetricks uses. Selawik comes from Microsoft's GitHub release. DXVK comes from a pinned `doitsujin/dxvk` release.
**Alternative:** winetricks. It doesn't cover sppc, t2embed or cmd.exe, it adds a moving dependency, and it gives no structured progress.

### D5. Prefix configuration via `wine reg add` batches
Registry changes (DLL overrides, Windows version, Direct3D/Direct2D values, FontSubstitutes, Wine graphics driver, `winemenubuilder.exe=d`) are applied with `wine reg add/delete` commands, as in the Bottles recipe. The Windows version is set through `HKLM\Software\Microsoft\Windows NT\CurrentVersion` the same way winecfg does it. Each logical group is one step.
**Alternative:** editing `user.reg`/`system.reg` directly. That's fragile while `wineserver` is running.

### D6. Active runner as a symlink, read by a POSIX shell wrapper
`~/.msoffice/runners/active` is a symlink to `runners/<tag>`, and `config.toml` mirrors it. The launch wrapper `~/.msoffice/bin/msoffice-launch` is a small generated `/bin/sh` script. It:
1. Sets `WINEPREFIX` and `WINEDLLOVERRIDES=winemenubuilder.exe=d`.
2. Fixes the permissions on `ClickToRunPackageLocker`.
3. Maps the app id to an EXE path.
4. Converts each argument with `winepath -w`.
5. Runs `exec runners/active/bin/wine <exe> <args>`.

Because it doesn't need the Rust binary, Office survives uninstalling the app (app-settings: Self-uninstall), and switching runners only moves the symlink.
**Alternative:** the Rust binary with a `launch` subcommand. Rejected because launchers would break if the app is removed or moved.

### D7. Dependency detection via `ldconfig -p` and a soname table
`ldconfig -p` output is parsed. Its `(libc6,x86-64)` and `(libc6)` tags separate 64-bit from 32-bit. Tools are found on `PATH`. `deps/maps.rs` maps each soname to a package per family. The table for the Arch family is verified in the container. The Fedora (`.i686`) and Debian (`:i386`) tables are written from package databases and marked untested.

Required sonames are the core set needed to start Wine, draw windows and run Office:
- `ld-linux*`
- libunwind (64-bit only)
- freetype, fontconfig, gnutls
- X11, Xext, Xrandr, Xcursor, Xrender, Xxf86vm, xkbcommon
- wayland-client, GL/EGL, vulkan
- pulse, asound, udev, dbus, krb5, gmp, gcrypt, cups

Optional sonames (ffmpeg 6, gstreamer, sane, gphoto2, OpenCL, pcap, openxr, capi) are not required.

Elevation uses `pkexec <pm> …`. If `pkexec` is missing, the app shows the command with a Copy button. For container testing only, the env var `LOIS_ELEVATE="sudo -n"` replaces `pkexec`; it is not documented in the UI.

### D8. Product picker is a static plan table
Product choices map plan names to Product IDs (office-deployment-config spec). The default is Microsoft 365 Family / Personal, because the target users are home Linux users and Soda's personal-account sign-in is the path #500 confirmed. The table also drives the channel rules: `Current` is fixed for 2024 Retail. The LicensingNext registry key uses the chosen Product ID.

### D9. XML built with `quick-xml` writer from a typed model
`DeploymentConfig` (serde, also saved in config.toml) is rendered to XML with fixed element order. The preview and the installed file both come from the same `render()` output, which guarantees the byte-identical preview.

Example (defaults):
```xml
<Configuration>
  <Add OfficeClientEdition="64" Channel="Current">
    <Product ID="O365HomePremRetail">
      <Language ID="MatchOS" Fallback="en-us" />
      <Language ID="en-us" />
      <ExcludeApp ID="Groove" />
      <ExcludeApp ID="Lync" />
      <ExcludeApp ID="Teams" />
    </Product>
  </Add>
  <Display Level="None" AcceptEULA="TRUE" />
  <Updates Enabled="FALSE" />
  <Logging Level="Standard" Path="C:\msoffice-logs" />
</Configuration>
```

### D10. Progress from the Click-to-Run log
While setup runs, the app polls (500 ms) `prefix/drive_c/users/*/AppData/Local/Temp/*.log` and `drive_c/msoffice-logs/*.log`. It decodes the newest files as UTF-16LE, reads them incrementally by byte offset, and matches `total progress is now ([0-9]+)`. Bottles' glob is `CPAK-*.log`, but "CPAK" is that environment's machine name. Wine names logs after the host's name, so the app matches any `.log` and doesn't depend on the prefix. Lines are also forwarded to the log panel.

### D10a. Office files downloaded natively, setup installs from the cache
Measured on 2026-10-19 in the `office` container: under Wine, Click-to-Run can't use BITS (`0x80040154` class not registered, then `0x80200003` "Unable to resume job"). It falls back to one HTTP connection at about 270 KiB/s, and an HTTP range request timeout (`0x8007274c`) stalled it for about 25 minutes. 32 % of the payload took 1 h 40 min. On the same line, 6 parallel connections from Linux get the whole line (about 2.3 MB/s).

So a new stage (`office/source.rs`) downloads the payload before setup:
- **Build:** `https://officecdn.microsoft.com/pr/<GUID>/Office/Data/v64.cab` (or `v32.cab`), read with `cabextract`. Its `VersionDescriptor.xml` lists `<AvailableBuild RestrictionType="Default" Version="…"/>`. Channel GUIDs: Current `492350f6-3a01-4f97-b9c0-c7c6ddf67d60`, MonthlyEnterprise `55336b82-a18d-4dd6-b5f6-9e5095c314a6`, SemiAnnual `7ffbc6bf-bc32-4f92-8982-f9dd17fd3114`.
- **Files** (the ones setup itself requested, seen in its log): `v64_<build>.cab`, `<build>/i640.cab`, `s640.cab`, `stream.x64.x-none.dat`, and per culture `s64<lcid>.cab`, `stream.x64.<culture>.dat`, plus the optional `i64<lcid>.cab`. `v64_<build>.cab` is also copied to `v64.cab`. For x86 the parts are `32`/`x86`. `MatchOS` uses the locale Wine sees (`LC_ALL`, `LC_MESSAGES`, `LANG`) plus `en-us`. A culture outside the LCID table is left to `AllowCdnFallback`.
- **Download:** `core::download::download_ranged`, 6 connections, 16 MiB chunks written in place into `<file>.part`; finished chunk ids go to `<file>.part.chunks`, whose first line is `<size> <ETag>`, so a changed remote file starts over. Each chunk is retried 6 times. The CDN publishes no plain hash (its ETag isn't the SHA-256), so files are checked by size; setup validates the signed cabs and streams itself.
- **Setup:** `SourcePath="Z:\home\…\.msoffice\cache\office-source\<channel>"`, `Version="<build>"`, `AllowCdnFallback="TRUE"`. The prefix's `Z:` → `/` link is created if missing.
- **Failure:** a network error fails the stage (Install again resumes). A CDN that doesn't match the expected layout logs the reason and lets setup download by itself, as before.

### D11. Desktop integration through XDG files only
- **Entries:** `~/.local/share/applications/msoffice-<app>.desktop` with `Exec=<home>/.msoffice/bin/msoffice-launch <app> %F` (absolute path), `StartupWMClass=<exe lowercase>`, `Categories=Office;`, and `MimeType=`.
- **Icons:** `wrestool -x -t 14` extracts the group icon from the EXE, `icotool -x` splits it into sizes, and the PNGs go to `hicolor/<n>x<n>/apps/msoffice-<app>.png`.
- **Defaults:** default-handler opt-in uses `xdg-mime default`. Before changing anything, the previous `xdg-mime query default` result is stored in the ledger.
- **Refresh:** `update-desktop-database` and `gtk-update-icon-cache` run only if they're installed.

### D12. UI: custom flat component kit on Slint primitives
The UI doesn't use Slint's `std-widgets` look for any control with a border. The kit lives in `ui/kit/`:
- NavPane, NavItem
- SettingsCard, Expander
- Button, AccentButton
- CheckBox, RadioButton, ComboBox (Rectangle plus PopupWindow), ToggleSwitch, LineEdit (wrapping a bare `TextInput`)
- ProgressBar (3 px), indeterminate bar
- InfoBar, ContentDialog
- ListRow with an overflow menu
- ScrollArea (Flickable plus a flat scrollbar)

`ui/theme.slint` holds a `Theme` global with light and dark palettes. Its `dark` property follows `Palette.color-scheme`, with an override from the config. `SLINT_STYLE` is pinned to `fluent` in `build.rs` so Linux never falls back to the Qt style. A unit test greps `ui/**/*.slint` and fails on `gradient`. The window icon and font are embedded: `import "../assets/Inter*.ttf"`.
**Alternative:** fork `std-widgets/fluent` and patch out the four gradient brushes. It would have to be re-merged on every Slint update, and it still lacks NavigationView, SettingsCard, InfoBar and Expander.

### D13. App icon
The icon is generated with Codex-image: a flat, original design (for example a document sheet with a downward install arrow on a solid tile) in the accent blue, with no Microsoft marks and no gradients. It is exported at 1024 px and downscaled with ImageMagick to 16–512. The PNGs are committed to `assets/icons/hicolor/` and embedded for self-install.

### D14. Testing strategy
- **Unit tests:** os-release and ldconfig parsing, soname maps, XML rendering (golden files), plan table rules, ledger add/remove, wrapper script rendering, C2R progress regex on UTF-16 samples.
- **Integration in `office`:** dependency check (and install with `LOIS_ELEVATE`), runner install/switch/remove, prefix setup, and an ODT run up to the progress phase. A full Office install and sign-in is a manual check done by the user with their own account.
- **Isolation:** all test runs use the container's `$HOME`. Nothing runs on the host.

## Risks / Trade-offs

- [Soda prerelease regressions break Office] → Runner management lets users go back to a known-good tag. The default stays pinned to `soda-11.0-32-experimental` until it is deliberately bumped.
- [Legacy redistributable URLs disappear (archive.org, Bottles proxy, windowsupdate)] → Checksums are pinned, so a mismatch or 404 gives a clear per-component error. The cache means one success is enough. The table is data-only, so URLs are easy to update.
- [The Office cache takes 3–3.5 GB of disk] → One build per channel is kept; older builds are deleted when a new one is downloaded. The README names the folder so users can delete it.
- [947 MB Win7 SP1 download is slow or interrupted] → Resumable `.part` downloads with Range requests, with progress and ETA shown.
- [Office auto-update or C2R background tasks change a working build] → Updates are off by default (`<Updates Enabled="FALSE"/>`), following the Bottles recipe.
- [Known Office bugs under Wine (Outlook classic, PowerPoint new slide, Activate button crash)] → "Known issues" notes on those apps in the form. The success banner says to sign in rather than click Activate. These aren't fixed in this change.
- [Wrong Product ID for the user's plan prevents activation] → Plan-named picker with a warning. The user can reinstall with another plan after "Reset prefix".
- [Fedora/Debian package maps are wrong] → Labelled untested. The dependency check after installing reports any still-missing sonames by name.
- [`pkexec` without a polkit agent (minimal window managers)] → Fallback: show the exact command to copy into a terminal.
- [Custom UI kit takes more work than std-widgets] → The kit is small (about 15 components). Slint's `TextInput`, `PopupWindow` and `Flickable` handle the hard parts.

## Migration Plan

There's nothing to migrate in v1. Rolling back means "Uninstall Office" plus "Remove Linux Office Installer Suite", which delete everything in the ledger and, optionally, `~/.msoffice/`.

## Open Questions

- **Wayland driver setting (resolved 2026-10-19):** Bottles' `wayland: true` (`bottles/backend/wine/regkeys.py`, `toggle_wayland_driver`) writes `HKCU\Software\Wine\Drivers` `Graphics` = `x11,wayland`. At launch, `apply_wayland_preferences` in `winecommand.py` adds `WINE_USE_EGL=1` when the session is Wayland and `WAYLAND_DISPLAY` is set; its other changes (`winex11.drv=d`, removing `DISPLAY`) apply only to Proton runners. The prefix step writes the same `Graphics` value, and the launch wrapper sets `WINE_USE_EGL=1` when `WAYLAND_DISPLAY` is set. `renderer: gl` is `HKCU\Software\Wine\Direct3D` `renderer` = `gl` (`set_renderer`).
- **DXVK version (resolved 2026-10-19):** pinned to **v3.1.1** (`dxvk-3.1.1.tar.gz`, SHA-256 `40565b4a…f54e608`), the newest release. Checked in a Soda 11.0-32 prefix in the container: with DXVK's `x64` DLLs in `system32`, `x32` in `syswow64` and `d3d8/d3d9/d3d10core/d3d11/dxgi=native`, a test program's `D3D11CreateDevice` succeeds through DXVK on Vulkan (`DXVK: v3.1.1`, feature level 11_0 on the NVIDIA GPU).
- **Corefonts selection:** whether `andale32` and `webdin32` are worth downloading, or arial/times/verdana/georgia/trebuc/courie/comic/impact are enough. The default is to mirror the Bottles list.
