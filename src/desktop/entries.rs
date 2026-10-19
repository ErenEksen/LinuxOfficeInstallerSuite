//! `~/.local/share/applications/msoffice-<app>.desktop` (desktop-integration: Desktop entries).

use std::io;
use std::path::{Path, PathBuf};

use super::icons::{self, IconSource};
use crate::core::Cancel;
use crate::core::config::write_atomic;
use crate::core::ledger::{Category, Ledger};
use crate::core::paths::Paths;
use crate::office::detect::App;

pub fn desktop_id(app: App) -> String {
    format!("msoffice-{}.desktop", app.id())
}

pub fn icon_name(app: App) -> String {
    format!("msoffice-{}", app.id())
}

pub fn entry_path(paths: &Paths, app: App) -> PathBuf {
    paths.applications().join(desktop_id(app))
}

/// Quote an `Exec` argument per the Desktop Entry spec when it needs it.
pub fn exec_quote(arg: &str) -> String {
    let needs = arg.chars().any(|c| c.is_whitespace() || "\"'\\><~|&;$*?#()`".contains(c));
    if !needs {
        return arg.to_string();
    }
    let mut s = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s.push('"');
    // The whole value is also a string value: backslashes are escaped once more.
    s.replace('\\', "\\\\")
}

pub fn render(paths: &Paths, app: App) -> String {
    let exec = format!("{} {} %F", exec_quote(&paths.launch_wrapper().to_string_lossy()), app.id());
    let mimes: String = app.mime_types().iter().map(|m| format!("{m};")).collect();
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name={name}\n\
         GenericName={generic}\n\
         Comment={name} running in Wine\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         StartupNotify=true\n\
         StartupWMClass={wm}\n\
         Categories={cats}\n\
         MimeType={mimes}\n",
        name = app.name(),
        generic = app.generic_name(),
        icon = icon_name(app),
        wm = app.exe().to_ascii_lowercase(),
        cats = app.categories(),
    )
}

/// Write the icon and desktop entry for `app`, recording both in the ledger.
pub fn install(
    paths: &Paths,
    ledger: &mut Ledger,
    app: App,
    exe: Option<&Path>,
    cancel: &Cancel,
) -> io::Result<IconSource> {
    let source = icons::install_app_icon(paths, ledger, &icon_name(app), exe, cancel)?;
    let path = entry_path(paths, app);
    ledger.record_file(&path, Category::Office)?;
    write_atomic(&path, render(paths, app).as_bytes())?;
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_entry() {
        let paths = Paths::for_home(Path::new("/home/u"));
        let e = render(&paths, App::Word);
        assert!(e.contains("Name=Microsoft Word\n"));
        assert!(e.contains("Exec=/home/u/.msoffice/bin/msoffice-launch word %F\n"));
        assert!(e.contains("Icon=msoffice-word\n"));
        assert!(e.contains("Categories=Office;WordProcessor;\n"));
        assert!(e.contains("StartupWMClass=winword.exe\n"));
        for m in ["wordprocessingml.document", "application/msword;", "application/rtf", "opendocument.text"] {
            assert!(e.contains(m), "{m}");
        }
    }

    #[test]
    fn quotes_paths_with_spaces() {
        assert_eq!(exec_quote("/home/u/.msoffice/bin/x"), "/home/u/.msoffice/bin/x");
        assert_eq!(exec_quote("/home/My Name/x"), "\"/home/My Name/x\"");
        assert_eq!(exec_quote("/a b/$x"), "\"/a b/\\\\$x\"");
    }

    #[test]
    fn every_entry_passes_desktop_file_validate() {
        if crate::core::proc::which("desktop-file-validate").is_none() {
            eprintln!("desktop-file-validate not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(&tmp.path().join("My Home"));
        let mut ledger = Ledger::load(&paths.ledger_file());
        for app in App::ALL {
            install(&paths, &mut ledger, app, None, &Cancel::new()).unwrap();
            let out = std::process::Command::new("desktop-file-validate").arg(entry_path(&paths, app)).output().unwrap();
            let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success() && text.trim().is_empty(), "{app:?}: {text}");
        }
        assert_eq!(ledger.files(Category::Office).count(), App::ALL.len() * (1 + icons::GENERIC.len()));
    }
}
