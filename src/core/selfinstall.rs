//! Installing and removing the app itself for the current user
//! (app-settings: Self-install, Self-uninstall).

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::config::write_atomic;
use super::ledger::{Category, Ledger};
use super::paths::{APP_ID, Paths};
use crate::desktop::entries::{self, exec_quote};
use crate::desktop::mime;
use crate::office::detect::App;
use crate::desktop::icons::{APP_ICON, install_pngs};

/// True when the running binary is the one in `~/.local/bin`.
pub fn is_installed_copy(paths: &Paths, exe: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(exe) == canon(&paths.installed_binary()) || exe.parent().map(canon) == Some(canon(&paths.local_bin()))
}

pub fn desktop_entry_path(paths: &Paths) -> PathBuf {
    paths.applications().join(format!("{APP_ID}.desktop"))
}

pub fn render_desktop_entry(paths: &Paths) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=Linux Office Installer Suite\n\
         GenericName=Office Installer\n\
         Comment=Install Microsoft 365 or Office 2024 with Wine\n\
         Exec={}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         StartupNotify=true\n\
         StartupWMClass={APP_ID}\n\
         Categories=Utility;\n\
         Keywords=Office;Wine;Microsoft;Installer;\n",
        exec_quote(&paths.installed_binary().to_string_lossy())
    )
}

/// Copy `exe` to `~/.local/bin`, install the icon and the desktop entry. Every file is
/// recorded in the ledger under [`Category::App`].
pub fn install(paths: &Paths, exe: &Path) -> io::Result<()> {
    let mut ledger = Ledger::load(&paths.ledger_file());
    let dest = paths.installed_binary();
    std::fs::create_dir_all(paths.local_bin())?;
    ledger.record_file(&dest, Category::App)?;
    let tmp = dest.with_extension("new");
    std::fs::copy(exe, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, &dest)?;

    let pngs: Vec<(u32, Vec<u8>)> = APP_ICON.iter().map(|(s, b)| (*s, b.to_vec())).collect();
    install_pngs(paths, &mut ledger, Category::App, APP_ID, &pngs)?;

    let entry = desktop_entry_path(paths);
    ledger.record_file(&entry, Category::App)?;
    write_atomic(&entry, render_desktop_entry(paths).as_bytes())?;
    crate::desktop::refresh_databases(paths);
    Ok(())
}

/// Remove the app's own files and its config directory. With `remove_office`, the Office
/// launchers, default handlers and `~/.msoffice/` go too; otherwise Office stays usable.
pub fn uninstall(paths: &Paths, remove_office: bool) -> io::Result<()> {
    if remove_office {
        crate::desktop::remove_office_integration(paths)?;
        let runner = paths.active_runner_link();
        if runner.exists() {
            crate::prefix::wine::Wine::new(&runner, paths.prefix()).kill_all();
        }
        match std::fs::remove_dir_all(&paths.root) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let mut ledger = Ledger::load(&paths.ledger_file());
    ledger.remove_category(Category::App)?;
    crate::desktop::refresh_databases(paths);
    match std::fs::remove_dir_all(&paths.config_dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove everything the app ever created: Office and `~/.msoffice/` (runners, prefix,
/// downloads, logs), the launchers, icons and file associations, the app's own binary, menu
/// entry and settings. After the ledger-driven removal, files with our exact names are swept
/// too, so a lost or older ledger leaves nothing behind.
pub fn remove_everything(paths: &Paths) -> io::Result<()> {
    uninstall(paths, true)?;
    sweep(paths)?;
    crate::desktop::refresh_databases(paths);
    Ok(())
}

fn sweep(paths: &Paths) -> io::Result<()> {
    let office_ids: Vec<String> = App::ALL.iter().map(|a| entries::desktop_id(*a)).collect();
    let mut files = vec![paths.installed_binary(), desktop_entry_path(paths)];
    files.extend(office_ids.iter().map(|id| paths.applications().join(id)));
    let mut icon_names: Vec<String> = App::ALL.iter().map(|a| entries::icon_name(*a)).collect();
    icon_names.push(APP_ID.to_string());
    if let Ok(dirs) = std::fs::read_dir(paths.icons_hicolor()) {
        for dir in dirs.flatten() {
            let apps = dir.path().join("apps");
            files.extend(icon_names.iter().map(|n| apps.join(format!("{n}.png"))));
        }
    }
    for f in files {
        match std::fs::remove_file(&f) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    let mimeapps = paths.mimeapps();
    if let Ok(text) = std::fs::read_to_string(&mimeapps) {
        let ids: Vec<&str> = office_ids.iter().map(String::as_str).collect();
        let cleaned = mime::strip_ids(&text, &ids);
        if cleaned != text {
            write_atomic(&mimeapps, cleaned.as_bytes())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_exe(dir: &Path) -> PathBuf {
        let exe = dir.join("Downloads/linux-office-installer-suite");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"\x7fELF fake").unwrap();
        exe
    }

    #[test]
    fn install_then_uninstall_keeping_office() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let exe = fake_exe(tmp.path());
        assert!(!is_installed_copy(&paths, &exe));
        install(&paths, &exe).unwrap();
        assert!(is_installed_copy(&paths, &paths.installed_binary()));
        assert_eq!(std::fs::read(paths.installed_binary()).unwrap(), b"\x7fELF fake");
        assert!(paths.icons_hicolor().join("256x256/apps/linux-office-installer-suite.png").exists());
        let entry = std::fs::read_to_string(desktop_entry_path(&paths)).unwrap();
        assert!(entry.contains(&format!("Exec={}\n", paths.installed_binary().display())));

        // Office data and launchers must survive "keep Office".
        std::fs::create_dir_all(paths.prefix()).unwrap();
        let office_entry = paths.applications().join("msoffice-word.desktop");
        let mut ledger = Ledger::load(&paths.ledger_file());
        ledger.record_file(&office_entry, Category::Office).unwrap();
        std::fs::write(&office_entry, "x").unwrap();

        uninstall(&paths, false).unwrap();
        assert!(!paths.installed_binary().exists());
        assert!(!desktop_entry_path(&paths).exists());
        assert!(!paths.icons_hicolor().join("256x256/apps/linux-office-installer-suite.png").exists());
        assert!(!paths.config_dir.exists());
        assert!(paths.prefix().exists() && office_entry.exists());
    }

    #[test]
    fn uninstall_with_office() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        install(&paths, &fake_exe(tmp.path())).unwrap();
        std::fs::create_dir_all(paths.prefix()).unwrap();
        uninstall(&paths, true).unwrap();
        assert!(!paths.root.exists());
        assert!(!paths.installed_binary().exists());
    }

    #[test]
    fn entry_validates() {
        if crate::core::proc::which("desktop-file-validate").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        install(&paths, &fake_exe(tmp.path())).unwrap();
        let out = std::process::Command::new("desktop-file-validate").arg(desktop_entry_path(&paths)).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && text.trim().is_empty(), "{text}");
    }

    #[test]
    fn remove_everything_sweeps_unrecorded_leftovers() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        install(&paths, &fake_exe(tmp.path())).unwrap();
        std::fs::create_dir_all(paths.prefix()).unwrap();
        // Leftovers from a lost ledger, plus files that are not ours.
        let word = paths.applications().join("msoffice-word.desktop");
        let other = paths.applications().join("org.gnome.TextEditor.desktop");
        let icon = paths.icons_hicolor().join("48x48/apps/msoffice-excel.png");
        let other_icon = paths.icons_hicolor().join("48x48/apps/firefox.png");
        std::fs::create_dir_all(icon.parent().unwrap()).unwrap();
        for f in [&word, &other, &icon, &other_icon] {
            std::fs::write(f, "x").unwrap();
        }
        std::fs::write(
            paths.mimeapps(),
            "[Default Applications]\napplication/msword=msoffice-word.desktop;\ntext/plain=org.gnome.TextEditor.desktop;\n\n[Added Associations]\napplication/pdf=msoffice-word.desktop;org.gnome.Papers.desktop;\n",
        )
        .unwrap();

        remove_everything(&paths).unwrap();
        assert!(!paths.root.exists() && !paths.config_dir.exists());
        assert!(!paths.installed_binary().exists() && !desktop_entry_path(&paths).exists());
        assert!(!word.exists() && !icon.exists());
        assert!(other.exists() && other_icon.exists(), "other apps' files stay");
        assert_eq!(
            std::fs::read_to_string(paths.mimeapps()).unwrap(),
            "[Default Applications]\ntext/plain=org.gnome.TextEditor.desktop;\n\n[Added Associations]\napplication/pdf=org.gnome.Papers.desktop;\n"
        );
    }
}
