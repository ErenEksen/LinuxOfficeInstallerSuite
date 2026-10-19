//! XDG desktop integration for the installed Office apps (design D11).

pub mod entries;
pub mod icons;
pub mod launcher;
pub mod mime;

use std::io;
use std::path::Path;

use crate::core::Cancel;
use crate::core::ledger::{Category, Ledger};
use crate::core::paths::Paths;
use crate::core::proc::{Cmd, capture, which};
use crate::office::detect::{self, App};

/// What [`install_office_integration`] did, for the log and the UI.
#[derive(Debug, Default)]
pub struct IntegrationReport {
    pub launchers: Vec<App>,
    pub generic_icons: Vec<App>,
}

/// Write the launch wrapper, one launcher (with icon) per installed app, and, when the user
/// opted in, the default handlers. Every file outside `~/.msoffice/` goes into the ledger.
pub fn install_office_integration(
    paths: &Paths,
    apps: &[App],
    default_handlers: bool,
    cancel: &Cancel,
) -> io::Result<IntegrationReport> {
    let office16 = detect::office16_dir(&paths.prefix())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Office installation found in the prefix"))?;
    launcher::install(paths, detect::office16_windows_path(&office16))?;
    let mut ledger = Ledger::load(&paths.ledger_file());
    crate::identity::install(paths, &mut ledger)?;
    let mut report = IntegrationReport::default();
    for &app in apps {
        let exe = detect::find_exe(&office16, app);
        let src = entries::install(paths, &mut ledger, app, exe.as_deref(), cancel)?;
        if src == icons::IconSource::Generic {
            report.generic_icons.push(app);
        }
        report.launchers.push(app);
    }
    if default_handlers {
        for &app in apps {
            mime::apply_defaults(&paths.mimeapps(), &mut ledger, &entries::desktop_id(app), app.mime_types())?;
        }
    }
    refresh_databases(paths);
    Ok(report)
}

/// Rewrite one launcher (Office apps page: Recreate launcher).
pub fn recreate_launcher(paths: &Paths, app: App, cancel: &Cancel) -> io::Result<()> {
    let office16 = detect::office16_dir(&paths.prefix())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no Office installation found in the prefix"))?;
    launcher::install(paths, detect::office16_windows_path(&office16))?;
    let mut ledger = Ledger::load(&paths.ledger_file());
    crate::identity::install(paths, &mut ledger)?;
    entries::install(paths, &mut ledger, app, detect::find_exe(&office16, app).as_deref(), cancel)?;
    refresh_databases(paths);
    Ok(())
}

/// Start an app through the wrapper, detached from this process.
pub fn launch(paths: &Paths, app: App, files: &[&Path]) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(paths.launch_wrapper())
        .arg(app.id())
        .args(files)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
}

/// Remove every Office launcher, icon and default-handler change recorded in the ledger.
pub fn remove_office_integration(paths: &Paths) -> io::Result<()> {
    let mut ledger = Ledger::load(&paths.ledger_file());
    mime::restore_defaults(&paths.mimeapps(), &mut ledger)?;
    ledger.remove_category(Category::Office)?;
    refresh_databases(paths);
    Ok(())
}

/// Refresh the menu and icon caches, when those tools are installed.
pub fn refresh_databases(paths: &Paths) {
    if which("update-desktop-database").is_some() && paths.applications().is_dir() {
        let _ = capture(&Cmd::new("update-desktop-database").arg(paths.applications()));
    }
    // KDE resolves link handlers (the sign-in helper's `ms-appx-web:`) from its own cache,
    // built from the session's directories, so only for the real ones (not in tests).
    if which("kbuildsycoca6").is_some() && paths.applications() == Paths::from_env().applications() {
        let _ = capture(&Cmd::new("kbuildsycoca6").arg("--noincremental"));
    }
    if which("gtk-update-icon-cache").is_some() && paths.icons_hicolor().join("index.theme").exists() {
        let _ = capture(&Cmd::new("gtk-update-icon-cache").args(["-f", "-t", "-q"]).arg(paths.icons_hicolor()));
    }
}

/// Live check of opt-in defaults with the real `xdg-mime`: `cargo test -- --ignored live_mime`.
#[cfg(test)]
mod live {
    use super::*;

    const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

    fn query() -> String {
        capture(&Cmd::new("xdg-mime").args(["query", "default", DOCX])).unwrap().1.trim().to_string()
    }

    #[test]
    #[ignore]
    fn live_mime_default_changes_and_is_restored() {
        let paths = Paths::from_env();
        let apps = paths.applications();
        std::fs::create_dir_all(&apps).unwrap();
        let other = apps.join("lois-test-writer.desktop");
        std::fs::write(&other, "[Desktop Entry]\nType=Application\nName=Test Writer\nExec=true %F\nMimeType=application/vnd.openxmlformats-officedocument.wordprocessingml.document;\n").unwrap();
        let (st, _) = capture(&Cmd::new("xdg-mime").args(["default", "lois-test-writer.desktop", DOCX])).unwrap();
        assert!(st.success());
        assert_eq!(query(), "lois-test-writer.desktop");

        // xdg-mime ignores a default whose Exec binary does not exist, so the wrapper must be there.
        let had_wrapper = launcher::exists(&paths);
        launcher::install(&paths, r"C:\Program Files\Microsoft Office\root\Office16").unwrap();
        let mut ledger = Ledger::load(&paths.ledger_file());
        entries::install(&paths, &mut ledger, App::Word, None, &Cancel::new()).unwrap();
        mime::apply_defaults(&paths.mimeapps(), &mut ledger, &entries::desktop_id(App::Word), App::Word.mime_types()).unwrap();
        eprintln!("after opt-in: {}", query());
        assert_eq!(query(), "msoffice-word.desktop");

        remove_office_integration(&paths).unwrap();
        eprintln!("after removal: {}", query());
        assert_eq!(query(), "lois-test-writer.desktop");
        assert!(!entries::entry_path(&paths, App::Word).exists());

        std::fs::remove_file(&other).unwrap();
        if !had_wrapper {
            std::fs::remove_file(paths.launch_wrapper()).unwrap();
        }
        let text = std::fs::read_to_string(paths.mimeapps()).unwrap_or_default();
        let cleaned = mime::edit_default(&text, DOCX, None);
        std::fs::write(paths.mimeapps(), cleaned).unwrap();
    }
}
