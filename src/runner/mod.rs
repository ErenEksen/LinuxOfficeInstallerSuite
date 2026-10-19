pub mod active;
pub mod github;
pub mod install;

use std::path::PathBuf;

use reqwest::blocking::Client;

use crate::core::Cancel;
use crate::core::config::Config;
use crate::core::paths::Paths;
use crate::core::proc::OpLog;
use github::{DEFAULT_RUNNER, Release};
use install::{Phase, RunnerError};

/// The oldest Soda build that runs Office: (major, minor, build) of `soda-11.0-32-…`.
const MIN_OFFICE_SODA: (u32, u32, u32) = (11, 0, 32);

/// Shown for runners too old for Office.
pub const TOO_OLD: &str = "Too old for Office (needs Soda 11.0-32 or later)";

/// Whether a runner tag (`soda-<major>.<minor>-<build>[-…]`) is Soda 11.0-32 or later.
/// Older builds, other runners and unreadable tags cannot run Office.
pub fn supports_office(tag: &str) -> bool {
    let parse = || -> Option<(u32, u32, u32)> {
        let rest = tag.strip_prefix("soda-")?;
        let (ver, tail) = rest.split_once('-')?;
        let (major, minor) = ver.split_once('.')?;
        let build = tail.split('-').next()?;
        Some((major.parse().ok()?, minor.parse().ok()?, build.parse().ok()?))
    };
    parse().is_some_and(|v| v >= MIN_OFFICE_SODA)
}

/// Make sure a runner is active. With no runner installed, the default runner is
/// downloaded and activated (wine-runner-management: Default runner). Returns the active
/// runner's directory (through the `active` symlink, so launchers follow switches).
pub fn ensure_runner(
    client: &Client,
    paths: &Paths,
    config: &mut Config,
    progress: &mut dyn FnMut(Phase, u64, Option<u64>),
    log: Option<&OpLog>,
    cancel: &Cancel,
) -> Result<PathBuf, RunnerError> {
    if active::reconcile(paths, config).is_none() {
        let installed = active::installed(paths);
        let tag = if installed.iter().any(|t| t == DEFAULT_RUNNER) {
            DEFAULT_RUNNER.to_string()
        } else if let Some(first) = installed.first() {
            first.clone()
        } else {
            if let Some(l) = log {
                l.line(&format!("no runner installed; installing the default {DEFAULT_RUNNER}"));
            }
            install::install_runner(client, paths, &Release::by_convention(DEFAULT_RUNNER), progress, log, cancel)?;
            DEFAULT_RUNNER.to_string()
        };
        active::set_active(paths, config, &tag).map_err(|e| match e {
            active::ActiveError::Io(io) => RunnerError::Io(io),
            other => RunnerError::Io(std::io::Error::other(other.to_string())),
        })?;
        if let Some(l) = log {
            l.line(&format!("active runner: {tag}"));
        }
    }
    Ok(paths.active_runner_link())
}

/// Live checks against the real `~/.msoffice` of the test system. Run one at a time:
/// `cargo test -- --ignored --test-threads=1 live_runner`.
#[cfg(test)]
mod live {
    use super::*;
    use crate::core::download::http_client;

    #[test]
    #[ignore]
    fn live_runner_1_default_installed_when_none() {
        let paths = Paths::from_env();
        assert!(active::installed(&paths).is_empty(), "start with an empty runners/ dir");
        let mut cfg = Config::default();
        let mut last = (None, 0);
        let dir = ensure_runner(&http_client(), &paths, &mut cfg, &mut |p, d, _| last = (Some(p), d), None, &Cancel::new())
            .unwrap();
        assert_eq!(dir, paths.active_runner_link());
        assert_eq!(active::active(&paths).as_deref(), Some(DEFAULT_RUNNER));
        assert_eq!(cfg.active_runner.as_deref(), Some(DEFAULT_RUNNER));
        assert!(dir.join("bin/wine").exists());
        eprintln!("last progress: {last:?}");
    }

    #[test]
    #[ignore]
    fn live_runner_2_second_runner_switch_and_remove() {
        let paths = Paths::from_env();
        let mut cfg = Config::default();
        let client = http_client();
        let releases = github::fetch_releases(&client, github::API_URL).unwrap();
        let second = releases.iter().find(|r| r.tag == "soda-11.0-31-experimental").unwrap();
        install::install_runner(&client, &paths, second, &mut |_, _, _| {}, None, &Cancel::new()).unwrap();
        assert!(active::installed(&paths).contains(&second.tag));
        active::set_active(&paths, &mut cfg, &second.tag).unwrap();
        assert_eq!(active::active(&paths).as_deref(), Some(second.tag.as_str()));
        let v = std::process::Command::new(paths.active_runner_link().join("bin/wine")).arg("--version").output().unwrap();
        eprintln!("active wine: {}", String::from_utf8_lossy(&v.stdout).trim());
        assert!(active::remove(&paths, &second.tag).is_err(), "active runner cannot be removed");
        active::set_active(&paths, &mut cfg, DEFAULT_RUNNER).unwrap();
        active::remove(&paths, &second.tag).unwrap();
        assert_eq!(active::installed(&paths), [DEFAULT_RUNNER]);
        assert_eq!(active::active(&paths).as_deref(), Some(DEFAULT_RUNNER));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_soda_11_0_32_and_later_run_office() {
        for ok in ["soda-11.0-32-experimental", "soda-11.0-33-experimental", "soda-11.1-1", "soda-12.0-2-experimental"] {
            assert!(supports_office(ok), "{ok}");
        }
        for old in ["soda-11.0-31-experimental", "soda-11.0-9-experimental", "soda-10.9-40", "soda-9.0-1", "wine-ge-8", "soda-x", ""] {
            assert!(!supports_office(old), "{old}");
        }
    }
}
