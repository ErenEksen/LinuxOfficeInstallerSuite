mod core;
mod deps;
mod desktop;
mod identity;
mod office;
mod prefix;
mod runner;
mod ui;

use std::fs::File;
use std::os::fd::AsRawFd;
use std::process::ExitCode;

use crate::core::config::Config;
use crate::core::paths::Paths;

slint::include_modules!();

const ROOT_ERROR: &str = "Linux Office Installer Suite must run as a normal user, not as root.\n\
Office is installed into your own home directory. The app asks for your password only when it\n\
installs system packages. Start it again without sudo.";

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("{ROOT_ERROR}");
        return ExitCode::from(1);
    }

    // The sign-in helper (`msoffice-identity`) is this binary too; it never opens the UI.
    let args: Vec<_> = std::env::args_os().collect();
    if let Some(code) = identity::run_cli(&args) {
        return code;
    }

    let paths = Paths::from_env();
    let _instance = match acquire_instance_lock(&paths) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            eprintln!("Linux Office Installer Suite is already running.");
            return ExitCode::from(1);
        }
        Err(e) => {
            eprintln!("Could not create {}: {e}", paths.config_dir.display());
            return ExitCode::from(1);
        }
    };

    let loaded = Config::load(&paths.config_file());
    let window = match AppWindow::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Could not start the user interface: {e}");
            return ExitCode::from(1);
        }
    };
    ui::bridge::setup(&window, paths, loaded.config, loaded.notice);
    match window.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("The user interface stopped with an error: {e}");
            ExitCode::from(1)
        }
    }
}

/// Hold an exclusive `flock` on the instance lock for the process lifetime.
/// `Ok(None)` means another instance holds it.
fn acquire_instance_lock(paths: &Paths) -> std::io::Result<Option<File>> {
    std::fs::create_dir_all(&paths.config_dir)?;
    let file = File::create(paths.instance_lock())?;
    // SAFETY: valid fd owned by `file`; the lock is released when it is closed.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(Some(file))
    } else {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(None) } else { Err(err) }
    }
}
