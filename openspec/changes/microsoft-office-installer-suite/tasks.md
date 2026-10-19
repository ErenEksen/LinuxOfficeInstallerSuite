# Tasks

All builds, installs and test runs happen inside the `office` distrobox (`distrobox enter office -- ...`, HOME=/home/axis/distrobox/office). Nothing runs on the host.

## 1. Project scaffolding and toolchain

- [x] 1.1 Install the build toolchain in the `office` container (`rust`, `cargo`, `base-devel`, `pkgconf`, `fontconfig`, `libxkbcommon`, `wayland`, `libx11`, `imagemagick`) and verify `cargo --version` and `rustc --version` run inside the container
- [x] 1.2 Create the Cargo binary crate `linux-office-installer-suite` with the module tree from design D1, plus `ui/`, `assets/` and `build.rs`. Pin `SLINT_STYLE=fluent` in `build.rs`. Verify `cargo build` succeeds and an empty Slint window opens from the container
- [x] 1.3 Add dependencies (`slint`, `slint-build`, `reqwest` blocking+rustls, `sha2`, `md-5`, `tar`, `xz2`, `serde`, `toml`, `quick-xml`, `directories`, `thiserror`, `log`/`env_logger`). Verify `cargo build` and `cargo test` pass with no warnings-as-errors failures
- [x] 1.4 Add `.gitignore`, `README.md` (purpose, legal note: no Microsoft binaries or license workarounds, sign in with your own account, test-container note), and verify the README renders and names every path under `~/.msoffice/`

## 2. Core foundation (paths, config, ledger, downloads, processes)

- [x] 2.1 Implement `core/paths.rs` (XDG config dir, `~/.msoffice/{runners,prefix,cache,bin,logs}`, applications/icons dirs) and verify with unit tests using a temporary HOME
- [x] 2.2 Implement `core/config.rs` (serde model: active runner, deployment choices, theme, default-handler flag, install state). Defaults on a missing file; a corrupt file is renamed to `config.toml.bak` with a notice flag. Verify unit tests cover missing, valid and corrupt files (app-settings: Config file location)
- [x] 2.3 Implement `core/ledger.rs` (record a written file, record a MIME default with its previous value, remove all entries in a category, persist in the config dir). Verify unit tests show removal deletes exactly the recorded files and nothing else (desktop-integration: Recorded file ledger)
- [x] 2.4 Implement `core/download.rs` (resumable `.part` download with Range, progress callback, cancel token, SHA-256 and MD5 verification, delete on mismatch). Verify unit tests against a local test HTTP server cover resume, mismatch and cancel
- [x] 2.5 Implement `core/proc.rs` (spawn with env, stream stdout and stderr lines to a callback and to a timestamped log in `~/.msoffice/logs/`, cancel kills the process group). Verify unit tests run `sh -c` scripts, capture lines and confirm the log file is written (app-settings: Operation logs)
- [x] 2.6 Implement the root refusal and single-instance check in `main.rs`. Verify that running as UID 0 (`sudo` in the container) prints the error and exits non-zero (app-settings: No system-wide changes without consent)

## 3. System dependencies

- [x] 3.1 Implement `deps/os_release.rs` (ID/ID_LIKE → arch, fedora, debian or unknown, plus support level). Verify unit tests with os-release fixtures for Arch, CachyOS, Manjaro, Fedora, Nobara, Debian, Ubuntu, Mint, openSUSE and Alpine
- [x] 3.2 Implement `deps/ldconfig.rs` (parse `ldconfig -p` into 64-bit and 32-bit soname sets) and tool detection on PATH. Verify unit tests with captured `ldconfig -p` output from the container before and after the lib32 install
- [x] 3.3 Write `deps/maps.rs`: the required and optional soname lists from design D7 and package names per family. Verify the Arch map in the container: uninstall nothing, but check that every mapped Arch package exists with `pacman -Si`. Mark the Fedora and Debian maps untested in code comments and UI strings
- [x] 3.4 Implement the 32-bit repo check (pacman.conf `[multilib]` active, `dpkg --print-foreign-architectures` contains i386, Fedora always OK) with instruction text. Verify unit tests with pacman.conf fixtures (missing, commented, active)
- [x] 3.5 Implement `deps/install.rs`: build one install command per family, elevate with `pkexec` (or `LOIS_ELEVATE` override), fall back to a copyable command, re-check afterwards. Verify in the container with `LOIS_ELEVATE="sudo -n"` by removing `cabextract` and confirming the app reinstalls it
- [x] 3.6 Implement the runtime self-check (runner `wine --version`, then `wineboot` in `~/.msoffice/cache/selfcheck-prefix` and run the 32-bit `syswow64\reg.exe query`). Verify it passes in the container, and fails with captured output when `LD_LIBRARY_PATH` tricks hide `ld-linux.so.2` in a test

## 4. Wine runner management

- [x] 4.1 Implement `runner/github.rs` (list `bottlesdevs/wine` releases with tag `soda-*` and an x86_64 asset, with prerelease flag and date; handle the API rate limit and offline). Verify unit tests with recorded API JSON, and a live call in the container lists `soda-11.0-32-experimental`
- [x] 4.2 Implement `runner/install.rs` (download the archive and SHA256SUMS, verify, extract to a temp dir, then rename atomically into `runners/<tag>/`, no partial dir on failure). Verify installing `soda-11.0-32-experimental` in the container, and a tampered SHA256SUMS fixture leaves no directory
- [x] 4.3 Implement `runner/active.rs` (the `runners/active` symlink plus config mirror, set active, refuse to remove the active runner, remove an inactive one). Verify unit tests on a temp tree, and in the container install a second runner (`soda-11.0-31-experimental`), switch to it, switch back, and remove it
- [x] 4.4 Make the default runner install automatically when the install pipeline finds no runner. Verify with an empty `~/.msoffice/runners/` that the pipeline installs and activates `soda-11.0-32-experimental`

## 5. Wine prefix setup

- [x] 5.1 Implement the `prefix/steps.rs` step engine with `.lois-state.toml` (done-step ids, skip on re-run, resume after interrupt). Verify unit tests with fake steps, including an interrupted run resuming at the right step
- [x] 5.2 Implement `prefix/wine.rs` helpers (env with WINEPREFIX, `winemenubuilder.exe=d`, `wine reg add/delete`, set Windows version, set DLL overrides, `wineserver -w/-k`). Verify in the container by reading the values back with `wine reg query`
- [x] 5.3 Port the redistributable recipe table (design D4): Win7 SP1 → gdiplus, sppc, t2embed (x86 and amd64); msxml6; riched20; msls31; Q811493 → cmd.exe, with Bottles' URLs and MD5s. Compute and pin the SHA-256 of each archive during this task. Verify each component extracts the expected files into `system32`/`syswow64` in the container prefix
- [x] 5.4 Implement the fonts steps: core fonts (Bottles list), Selawik with FontSubstitutes in both registry hives. Verify font files exist in `drive_c/windows/Fonts` and the substitutes read back with `wine reg query`
- [x] 5.5 Implement the graphics steps: DXVK (resolve the design open question: pin the newest DXVK release that works), renderer=gl, Direct3D `MaxVersionGL=0x30002`, delete Direct2D `max_version_factory`, Wayland driver setting (resolve the design open question from Bottles' launch code). Record both decisions in design.md and verify by reading back the registry values
- [x] 5.6 Assemble the full prefix setup pipeline in recipe order (win7 → redists → overrides → fonts → graphics → win10). Verify a full run in the container finishes, `wine winver` reports Windows 10, a second run makes no changes, and no Wine-generated `.desktop` file appears
- [x] 5.7 Implement "Reset prefix" (delete the prefix and ledger launchers, keep runners and cache). Verify in the container that runners and cache remain after a reset

## 6. Office deployment config

- [x] 6.1 Implement the plan table (design D8 / spec Product choices) and channel rules (2024 Retail locks Current; Microsoft 365 allows Current, MonthlyEnterprise and SemiAnnual). Verify unit tests for each plan → Product ID and the channel lock
- [x] 6.2 Implement `office/config_xml.rs` (typed model → XML via quick-xml, fixed order, ExcludeApp mapping OneDrive→Groove and Skype→Lync, MatchOS plus en-us, Updates FALSE default, Display level, Logging, AcceptEULA). Verify golden-file tests for the defaults, every plan and every excluded app, plus a test that no `PIDKEY`, `AUTOACTIVATE` or KMS strings ever appear
- [x] 6.3 Implement the architecture-mismatch guard (read the installed edition from the prefix registry `ClickToRun\Configuration\Platform`). Verify a unit test with a fixture registry value blocks a 32-bit config over a 64-bit install
- [x] 6.4 Write the config to `~/.msoffice/Configuration.xml` and `drive_c/Configuration.xml` from the same rendered bytes. Verify a test asserts both files are byte-identical to the preview string

## 7. Office installation pipeline

- [x] 7.1 Implement `office/odt.rs` (download officecdn `setup.exe`, check the `MZ`/PE header, cache it). Verify in the container that it downloads and the PE check rejects a fixture HTML file
- [x] 7.2 Implement the ClickToRunSvc registration step (`sc create …`, exit codes 0 or 49 count as success). Verify in the container that running it twice succeeds both times
- [x] 7.3 Implement `office/progress.rs` (poll Temp and Logging `*.log`, UTF-16LE incremental decode, regex `total progress is now ([0-9]+)`, forward lines). Verify unit tests with UTF-16LE sample logs, including a split read across chunk boundaries
- [x] 7.4 Implement `office/pipeline.rs` with the stages from the spec (deps → runner → self-check → prefix → ODT → service → setup → post-fixes → detect → launchers), stage status, cancel (`wineserver -k`), and failure with exit code and log path. Verify in the container that a run reaches the setup stage with live percentage updates, and cancelling mid-setup stops all Wine processes (`pgrep -f wineserver` is empty)
- [x] 7.4a Download the Office files natively before setup (design D10a): `download_ranged` (parallel Range chunks, resumable, retried), `office/source.rs` (build from `v64.cab`, file list, cultures, cache under `cache/office-source/<channel>`, prune old builds), the "Download Office files" stage, and `SourcePath`/`Version`/`AllowCdnFallback` in the installed XML. Verify unit tests (ranged download, resume, ETag change, cancel, file list, cultures, XML), `live_cdn_layout` for every channel and edition, and a timed `live_prepare` run that reaches the line speed
- [ ] 7.4b Run a full install from the local source in the container and confirm setup reads the cache (no CDN download in the Click-to-Run log) and finishes
- [x] 7.5 Implement the post-install fixes (copy AppvIsvSubsystems64.dll and C2R64.dll into Office16, sppc builtin,native, LicensingNext\<ProductID>=2). Verify with unit tests on a fake prefix tree, and with registry read-back after a real install
- [x] 7.6 Implement `office/detect.rs` (scan Office16 for the 7 EXEs). Verify unit tests on fake trees for a full and a partial install
- [x] 7.7 Implement "Uninstall Office" (remove ledger launchers, icons and MIME defaults; delete the prefix optionally). Verify on a fake install in the container that only ledger files are removed and the prefix deletion follows the user's choice

## 8. Desktop integration

- [x] 8.1 Implement `desktop/launcher.rs`: render `~/.msoffice/bin/msoffice-launch` (POSIX sh: WINEPREFIX, menubuilder off, locker chmod 0770, app id → EXE, `winepath -w` per arg, exec `runners/active/bin/wine`). Verify `shellcheck` is clean (install `shellcheck` in the container) and a unit test checks the rendered script for each app id
- [x] 8.2 Implement `desktop/icons.rs` (`wrestool -x -t14` → `icotool -x` → hicolor PNG sizes named `msoffice-<app>.png`, generic icon fallback). Verify against a real EXE from the container prefix (e.g. `notepad.exe` before Office exists), and that the fallback is used for a file with no icons
- [x] 8.3 Implement `desktop/entries.rs` (desktop entry per detected app: Exec with absolute wrapper path and `%F`, Icon, Categories, StartupWMClass, MimeType lists from the spec; ledger recording; `update-desktop-database` if present). Verify `desktop-file-validate` passes on every generated entry in the container
- [x] 8.4 Implement `desktop/mime.rs` opt-in defaults (query and store previous defaults in the ledger, `xdg-mime default`, restore on removal). Verify in the container that `xdg-mime query default application/vnd.openxmlformats-officedocument.wordprocessingml.document` changes and is restored after removal

## 9. App icon and branding

- [x] 9.1 Generate the original app icon with Codex-image (flat, solid colours, accent blue, no Microsoft marks, no gradients; e.g. a document tile with a download arrow) at 1024 px. Verify by viewing the image and confirming it contains no gradient or trademark
- [x] 9.2 Export hicolor sizes 16, 32, 48, 64, 128, 256 and 512 with ImageMagick into `assets/icons/hicolor/`, add a generic document icon for the launcher fallback, and add the Inter font files to `assets/fonts/` with their OFL license. Verify all files exist at the right pixel sizes (`identify`)

## 10. UI component kit and theme

- [x] 10.1 Implement `ui/theme.slint` (Theme global with light and dark palettes from the installer-ui spec, follows `Palette.color-scheme`, with a config override) and import the Inter font. Verify both themes render, and switching at runtime updates the whole window without a restart
- [x] 10.2 Implement the kit controls: Button, AccentButton, CheckBox, RadioButton, ToggleSwitch, LineEdit, ComboBox (PopupWindow), ProgressBar (determinate and indeterminate). Verify on a kit gallery page run in the container, in both themes
- [x] 10.3 Implement the kit layout pieces: NavPane/NavItem (accent indicator, compact icons-only mode below 1008 px), SettingsCard, Expander, InfoBar, ContentDialog (safe default Cancel), ListRow with an overflow menu, ScrollArea. Verify on the gallery page, including the window resize collapse
- [x] 10.4 Add the no-gradient guard test (cargo test greps `ui/**/*.slint` for `gradient`, and lists every `std-widgets` import, allowing only ones checked to be gradient-free). Verify it fails when a `@linear-gradient` is added temporarily and passes otherwise

## 11. UI pages and Rust bridge

- [x] 11.1 Implement `ui/app.slint` (window, icon, NavPane with Install, Progress & Log, Office apps, Wine runtimes, Settings) and `ui/bridge.rs` (models, callbacks, worker thread with `invoke_from_event_loop`, design D2). Verify navigation between all pages in the container
- [x] 11.2 Implement the Install page (dependency status card with support-level label, untested note and Install packages action; the Product & release, Apps (known-issue notes), Language and Advanced cards; plan warning; Show XML dialog; Save as…; Install button). Verify the form round-trips through config.toml across restarts and that Show XML matches the installed file
- [x] 11.3 Implement the Progress & Log page (stage list, progress bar, Cancel with confirmation, collapsible live log, result InfoBar with the sign-in hint). Verify in the container during a real pipeline run
- [ ] 11.4 Implement the Office apps page (detected apps with Launch and Recreate launcher, Uninstall Office dialog with a keep/delete prefix choice). Verify Launch starts the wrapper and Recreate rewrites the entry
- [x] 11.5 Implement the Wine runtimes page (installed list with an active badge, Set as active, Remove disabled for the active runner, an Install new version dialog listing GitHub releases with download progress). Verify install, switch and remove from the UI in the container
- [x] 11.6 Implement the Settings page (theme Light/Dark/System, default-handler toggle, Reset prefix, Open logs folder, Remove Linux Office Installer Suite with the separate Office data question, About with legal note). Verify each action in the container
- [x] 11.7 Implement the self-install offer (shown when running outside `~/.local/bin`; copy the binary, install the icon and own desktop entry, all ledger-recorded) and self-uninstall. Verify in the container by running the binary from `~/Downloads`, accepting, and finding `linux-office-installer-suite.desktop` passing `desktop-file-validate`
- [x] 11.8 Add "Remove everything" to Settings (a confirmation dialog listing what goes; closes Office, deletes `~/.msoffice`, the Office integration with defaults restored, the app's own files and config, then sweeps leftovers with our exact names; runs off the UI thread and quits when done). Verify with a unit test on a fake home and in the container
- [x] 11.9 Scrolling: mouse drags must not pan pages (wheel, touch and the scrollbar only), the log's scrollbar can be dragged inside the scrolling page, clicking the scrollbar track jumps there, and the log follows new lines only while it is scrolled to the end. Verify in the container with xdotool
- [x] 11.10 Show the app name as "Linux Office Installer Suite" (window title, menu entry, dialogs, messages, docs). Verify the desktop entry still passes `desktop-file-validate`

## 12. End-to-end verification in the `office` container

- [ ] 12.1 From a clean container state (remove `~/.msoffice`, the app config and ledger files, and the `~/spike` leftovers), run the release binary and complete Install with default settings up to a finished Office setup. Verify that all stages are green, the 5 default apps are detected, launchers validate, and `~/.msoffice` matches the documented layout
- [ ] 12.2 Ask the user to sign in with their own Microsoft account inside Word from the generated launcher, and to open a `.docx` from the file manager via "Open with". Record the result (works or known issue) in README's compatibility section
- [ ] 12.3 Run uninstall Office, then remove the app. Verify that nothing from the ledger remains in `~/.local/share/applications`, `~/.local/share/icons` or `~/.local/bin`, and that the host home outside the container was never written (`find /home/axis -newer <marker> -not -path '/home/axis/distrobox/*'` shows no app files)
