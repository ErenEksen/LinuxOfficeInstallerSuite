//! Microsoft account sign-in for Office under Soda runners.
//!
//! Soda's Windows identity APIs (the ones Office's Sign in button uses) forward requests to
//! a broker on the Linux side, found through `SODA_IDENTITY_BRIDGE_SOCKET`. Without it,
//! sign-in fails at once. Bottles runs such a broker for its bottles; this module is ours:
//!
//! - `msoffice-identity ensure` starts the bridge if needed and prints its socket; the
//!   launch wrapper exports it before starting Office.
//! - `msoffice-identity serve` is the bridge itself ([`server`]).
//! - `msoffice-identity callback URI` is the `ms-appx-web:` URL handler that brings the
//!   browser sign-in back ([`provider`]).
//!
//! `msoffice-identity` is a copy of this binary in `~/.msoffice/bin`, so sign-in keeps
//! working when the installer app is removed.

pub mod protocol;
pub mod provider;
pub mod server;
pub mod store;

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::core::config::write_atomic;
use crate::core::ledger::{Category, Ledger};
use crate::core::paths::{APP_ID, Paths};
use crate::desktop::entries::exec_quote;
use crate::desktop::mime;

pub const HELPER_NAME: &str = "msoffice-identity";
pub const HANDLER_ID: &str = "msoffice-identity.desktop";
const HANDLER_MIME: &str = "x-scheme-handler/ms-appx-web";

/// Private per-user directory for the sockets: `$XDG_RUNTIME_DIR/linux-office-installer-suite`.
pub fn runtime_dir() -> io::Result<PathBuf> {
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute());
    let dir = match base {
        Some(b) => b.join(APP_ID),
        None => std::env::temp_dir().join(format!("{APP_ID}-{uid}")),
    };
    std::fs::create_dir_all(&dir)?;
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid {
        return Err(io::Error::other(format!("{} is not a private directory", dir.display())));
    }
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// The bridge socket for this `~/.msoffice`.
pub fn bridge_socket(dir: &Path, paths: &Paths) -> PathBuf {
    let hash = hex::encode(Sha256::digest(paths.root.as_os_str().as_encoded_bytes()));
    dir.join(format!("bridge-{}.sock", &hash[..16]))
}

pub fn helper_path(paths: &Paths) -> PathBuf {
    paths.bin().join(HELPER_NAME)
}

pub fn handler_path(paths: &Paths) -> PathBuf {
    paths.applications().join(HANDLER_ID)
}

pub fn render_handler(paths: &Paths) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name=Microsoft sign-in for Office\n\
         Comment=Returns a Microsoft account sign-in from the browser to Office running in Wine\n\
         Exec={} callback %u\n\
         Terminal=false\n\
         NoDisplay=true\n\
         MimeType={HANDLER_MIME};\n",
        exec_quote(&helper_path(paths).to_string_lossy()),
    )
}

/// Copy this binary to `~/.msoffice/bin/msoffice-identity`, write the `ms-appx-web:` URL
/// handler and make it the default for that scheme (recorded in the ledger).
pub fn install(paths: &Paths, ledger: &mut Ledger) -> io::Result<()> {
    let helper = helper_path(paths);
    let exe = std::env::current_exe()?;
    if std::fs::canonicalize(&exe).ok() != std::fs::canonicalize(&helper).ok() {
        let tmp = helper.with_extension("tmp");
        std::fs::create_dir_all(paths.bin())?;
        std::fs::copy(&exe, &tmp)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&tmp, &helper)?;
    }
    let entry = handler_path(paths);
    ledger.record_file(&entry, Category::Office)?;
    write_atomic(&entry, render_handler(paths).as_bytes())?;
    mime::apply_defaults(&paths.mimeapps(), ledger, HANDLER_ID, &[HANDLER_MIME])?;
    // The portal belongs to the session, so only for the session's own home (not in tests).
    if paths.mimeapps() == Paths::from_env().mimeapps() {
        forget_portal_choice();
    }
    Ok(())
}

/// The desktop portal (which browsers such as Chromium use to open links) remembers the
/// app last used for a link type and keeps using it over the system default. A choice
/// recorded before the helper existed (a text editor, say) would swallow the sign-in, so
/// drop it. Only this link type is touched; failures are fine (no portal running).
fn forget_portal_choice() {
    use crate::core::proc::{Cmd, capture, which};
    if which("gdbus").is_none() {
        return;
    }
    let _ = capture(&Cmd::new("gdbus").args([
        "call",
        "--session",
        "--dest",
        "org.freedesktop.impl.portal.PermissionStore",
        "--object-path",
        "/org/freedesktop/impl/portal/PermissionStore",
        "--method",
        "org.freedesktop.impl.portal.PermissionStore.Delete",
        "desktop-used-apps",
        HANDLER_MIME,
    ]));
}

/// Tell the user about a failure in a process that has no terminal (the URL handler).
fn notify(message: &str) {
    eprintln!("{HELPER_NAME}: {message}");
    let _ = std::process::Command::new("notify-send")
        .args(["--app-name=Microsoft Office", "Microsoft sign-in failed", message])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run `msoffice-identity <command>` (or `linux-office-installer-suite identity <command>`).
/// `None` when the arguments are not an identity command.
pub fn run_cli(args: &[OsString]) -> Option<ExitCode> {
    let invoked_as_helper = args
        .first()
        .and_then(|a| Path::new(a).file_name())
        .is_some_and(|n| n == HELPER_NAME);
    let rest: Vec<String> = if invoked_as_helper {
        args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect()
    } else if args.get(1).is_some_and(|a| a == "identity") {
        args.iter().skip(2).map(|a| a.to_string_lossy().into_owned()).collect()
    } else {
        return None;
    };
    let fail = |msg: String| {
        eprintln!("{HELPER_NAME}: {msg}");
        Some(ExitCode::from(1))
    };
    if !matches!(rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice(), ["ensure"] | ["serve"] | ["callback", _]) {
        eprintln!("usage: {HELPER_NAME} <ensure|serve|callback URI>");
        return Some(ExitCode::from(2));
    }
    let paths = Paths::from_env();
    let dir = match runtime_dir() {
        Ok(d) => d,
        Err(e) => return fail(format!("no runtime directory: {e}")),
    };
    match rest.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["ensure"] => {
            let socket = bridge_socket(&dir, &paths);
            let mut serve = std::process::Command::new(std::env::current_exe().unwrap_or_else(|_| helper_path(&paths)));
            if !invoked_as_helper {
                serve.arg("identity");
            }
            serve.arg("serve").stdout(std::process::Stdio::null());
            let log = std::fs::OpenOptions::new().create(true).append(true).open(paths.logs().join("identity-bridge.log"));
            serve.stderr(log.map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null()));
            match server::ensure(&socket, &mut serve) {
                Ok(s) => {
                    println!("{}", s.display());
                    Some(ExitCode::SUCCESS)
                }
                Err(e) => fail(e.to_string()),
            }
        }
        ["serve"] => {
            let store = store::Store::new(store::SecretKv);
            let provider = provider::Provider::new(store, provider::ReqwestHttp::new(), provider::XdgOpen, dir.clone());
            let server = Arc::new(server::Server { provider });
            match server.serve(&bridge_socket(&dir, &paths)) {
                Ok(()) => Some(ExitCode::SUCCESS),
                Err(e) => fail(e.to_string()),
            }
        }
        ["callback", uri] => match provider::forward_callback(&dir, uri) {
            Ok(()) => Some(ExitCode::SUCCESS),
            Err(e) => {
                notify(&e);
                Some(ExitCode::from(1))
            }
        },
        _ => unreachable!("checked above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_identity_commands_are_taken() {
        assert!(run_cli(&os(&["/usr/bin/linux-office-installer-suite"])).is_none());
        assert!(run_cli(&os(&["linux-office-installer-suite", "--help"])).is_none());
        assert_eq!(run_cli(&os(&["/x/msoffice-identity", "bogus"])), Some(ExitCode::from(2)));
        assert_eq!(run_cli(&os(&["linux-office-installer-suite", "identity"])), Some(ExitCode::from(2)));
    }

    #[test]
    fn handler_entry() {
        let paths = Paths::for_home(Path::new("/home/u"));
        let e = render_handler(&paths);
        assert!(e.contains("Exec=/home/u/.msoffice/bin/msoffice-identity callback %u\n"));
        assert!(e.contains("MimeType=x-scheme-handler/ms-appx-web;\n"));
        assert!(e.contains("NoDisplay=true\n"));
        let other = Paths::for_home(Path::new("/home/v"));
        let dir = Path::new("/run/user/1000/x");
        assert_ne!(bridge_socket(dir, &paths), bridge_socket(dir, &other), "one bridge per ~/.msoffice");
    }

    #[test]
    fn handler_passes_desktop_file_validate() {
        if crate::core::proc::which("desktop-file-validate").is_none() {
            eprintln!("desktop-file-validate not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join(HANDLER_ID);
        std::fs::write(&f, render_handler(&Paths::for_home(&tmp.path().join("My Home")))).unwrap();
        let out = std::process::Command::new("desktop-file-validate").arg(&f).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && text.trim().is_empty(), "{text}");
    }
}
