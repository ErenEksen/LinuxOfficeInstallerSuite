//! 32-bit repository check and privileged package installation
//! (system-dependencies: 32-bit repository check, Privileged package installation).

use std::path::Path;

use super::os_release::Family;
use crate::core::Cancel;
use crate::core::proc::{Cmd, OpLog, ProcError, Stream, capture, run, which};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoStatus {
    /// Enabled, or not applicable for this family.
    Enabled,
    /// Disabled: the text tells the user exactly how to enable it. We never edit it ourselves.
    Disabled { instructions: String },
}

pub const MULTILIB_INSTRUCTIONS: &str = "The 32-bit package repository [multilib] is not enabled.\n\
To enable it, open /etc/pacman.conf as root and remove the # in front of these two lines:\n\
\n    [multilib]\n    Include = /etc/pacman.d/mirrorlist\n\n\
Then update the package database with:\n\n    sudo pacman -Syu\n\nand choose Check again.";

pub const I386_INSTRUCTIONS: &str = "The 32-bit (i386) architecture is not enabled for apt.\n\
Enable it in a terminal with:\n\n    sudo dpkg --add-architecture i386\n    sudo apt update\n\nand choose Check again.";

/// True if pacman.conf has an active (uncommented) `[multilib]` section.
pub fn pacman_multilib_enabled(conf: &str) -> bool {
    conf.lines().any(|l| l.trim() == "[multilib]")
}

/// True if `dpkg --print-foreign-architectures` output lists i386.
pub fn dpkg_has_i386(foreign: &str) -> bool {
    foreign.split_whitespace().any(|a| a == "i386")
}

pub fn check_32bit_repo(family: Family) -> RepoStatus {
    check_32bit_repo_with(family, Path::new("/etc/pacman.conf"), || {
        capture(&Cmd::new("dpkg").arg("--print-foreign-architectures")).map(|(_, out)| out).unwrap_or_default()
    })
}

pub fn check_32bit_repo_with(family: Family, pacman_conf: &Path, dpkg_foreign: impl Fn() -> String) -> RepoStatus {
    let enabled = match family {
        Family::Arch => pacman_multilib_enabled(&std::fs::read_to_string(pacman_conf).unwrap_or_default()),
        Family::Debian => dpkg_has_i386(&dpkg_foreign()),
        // Fedora ships i686 packages in the main repos; unknown distros install by hand.
        Family::Fedora | Family::NixOS | Family::Unknown => true,
    };
    if enabled {
        return RepoStatus::Enabled;
    }
    let instructions = match family {
        Family::Arch => MULTILIB_INSTRUCTIONS,
        _ => I386_INSTRUCTIONS,
    };
    RepoStatus::Disabled { instructions: instructions.into() }
}

/// The package-manager command (without elevation) that installs `packages`.
pub fn install_command(family: Family, packages: &[&str]) -> Option<Vec<String>> {
    if packages.is_empty() {
        return None;
    }
    let pkgs = packages.iter().map(|p| p.to_string());
    let cmd: Vec<String> = match family {
        Family::Arch => ["pacman", "-S", "--needed", "--noconfirm"].iter().map(|s| s.to_string()).chain(pkgs).collect(),
        // UNTESTED
        Family::Fedora => ["dnf", "install", "-y"].iter().map(|s| s.to_string()).chain(pkgs).collect(),
        // UNTESTED. One elevated command, so the index refresh goes through `sh -c`.
        Family::Debian => vec![
            "sh".into(),
            "-c".into(),
            format!("apt-get update && apt-get install -y {}", packages.join(" ")),
        ],
        Family::NixOS | Family::Unknown => return None,
    };
    Some(cmd)
}

/// How to gain root for the package command: `pkexec`, or `LOIS_ELEVATE` (e.g. `sudo -n`)
/// when set. `None` when nothing is available; the UI then shows the command to copy.
pub fn elevation() -> Option<Vec<String>> {
    if let Some(v) = std::env::var_os("LOIS_ELEVATE") {
        let parts: Vec<String> = v.to_string_lossy().split_whitespace().map(String::from).collect();
        if !parts.is_empty() {
            return Some(parts);
        }
    }
    which("pkexec").map(|_| vec!["pkexec".into()])
}

/// The command the user can paste into a terminal themselves.
pub fn copyable_command(cmd: &[String]) -> String {
    let quoted: Vec<String> = cmd
        .iter()
        .map(|a| if a.contains(char::is_whitespace) || a.contains('&') { format!("'{}'", a.replace('\'', "'\\''")) } else { a.clone() })
        .collect();
    format!("sudo {}", quoted.join(" "))
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no way to gain administrator rights (pkexec is not installed). Run this command in a terminal instead:\n{0}")]
    NoElevation(String),
    #[error("the package installation failed ({status}). Run this command in a terminal to see the details:\n{command}")]
    Failed { status: String, command: String },
    #[error(transparent)]
    Proc(#[from] ProcError),
}

/// Run the elevated install command. Output streams to `on_line` and the log.
pub fn run_install(
    family: Family,
    packages: &[&str],
    log: &OpLog,
    on_line: &mut dyn FnMut(Stream, &str),
    cancel: &Cancel,
) -> Result<(), InstallError> {
    let Some(cmd) = install_command(family, packages) else { return Ok(()) };
    let copy = copyable_command(&cmd);
    let Some(elevate) = elevation() else { return Err(InstallError::NoElevation(copy)) };
    let mut full = Cmd::new(&elevate[0]).args(&elevate[1..]).args(&cmd);
    if family == Family::Debian {
        full = full.env("DEBIAN_FRONTEND", "noninteractive");
    }
    let out = run(&full, Some(log), on_line, cancel)?;
    if out.status.success() {
        Ok(())
    } else {
        // pkexec: 126 = authentication dismissed, 127 = not authorised / no agent.
        let status = match out.status.code() {
            Some(126) => "authentication was cancelled".to_string(),
            Some(127) => "authentication failed or no polkit agent is running".to_string(),
            _ => crate::core::proc::describe(out.status),
        };
        Err(InstallError::Failed { status, command: copy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multilib_detection() {
        let missing = "[options]\nArchitecture = auto\n\n[core]\nInclude = /etc/pacman.d/mirrorlist\n";
        let commented = format!("{missing}\n#[multilib]\n#Include = /etc/pacman.d/mirrorlist\n");
        let active = format!("{missing}\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n");
        let testing_only = format!("{missing}\n[multilib-testing]\nInclude = /etc/pacman.d/mirrorlist\n");
        assert!(!pacman_multilib_enabled(missing));
        assert!(!pacman_multilib_enabled(&commented));
        assert!(pacman_multilib_enabled(&active));
        assert!(!pacman_multilib_enabled(&testing_only));

        let tmp = tempfile::tempdir().unwrap();
        let conf = tmp.path().join("pacman.conf");
        std::fs::write(&conf, &commented).unwrap();
        let st = check_32bit_repo_with(Family::Arch, &conf, String::new);
        assert!(matches!(st, RepoStatus::Disabled { ref instructions } if instructions.contains("[multilib]")));
        std::fs::write(&conf, &active).unwrap();
        assert_eq!(check_32bit_repo_with(Family::Arch, &conf, String::new), RepoStatus::Enabled);
    }

    #[test]
    fn dpkg_and_fedora() {
        let none = Path::new("/nonexistent");
        assert!(matches!(check_32bit_repo_with(Family::Debian, none, || "amd64\n".into()), RepoStatus::Disabled { .. }));
        assert_eq!(check_32bit_repo_with(Family::Debian, none, || "i386\n".into()), RepoStatus::Enabled);
        assert_eq!(check_32bit_repo_with(Family::Fedora, none, String::new), RepoStatus::Enabled);
    }

    #[test]
    fn commands_per_family() {
        assert_eq!(
            install_command(Family::Arch, &["cabextract", "lib32-glibc"]).unwrap(),
            ["pacman", "-S", "--needed", "--noconfirm", "cabextract", "lib32-glibc"]
        );
        assert_eq!(install_command(Family::Fedora, &["glibc.i686"]).unwrap(), ["dnf", "install", "-y", "glibc.i686"]);
        let deb = install_command(Family::Debian, &["libc6:i386", "cabextract"]).unwrap();
        assert_eq!(deb[2], "apt-get update && apt-get install -y libc6:i386 cabextract");
        assert!(install_command(Family::Unknown, &["x"]).is_none());
        assert!(install_command(Family::Arch, &[]).is_none());
        assert_eq!(copyable_command(&deb), "sudo sh -c 'apt-get update && apt-get install -y libc6:i386 cabextract'");
    }

    /// Live check on the test system: `sudo pacman -R cabextract`, then
    /// `LOIS_ELEVATE="sudo -n" cargo test -- --ignored reinstalls_missing_tool`.
    #[test]
    #[ignore]
    fn reinstalls_missing_tool() {
        let before = crate::deps::check();
        assert!(before.missing.iter().any(|m| m.name == "cabextract"), "remove cabextract first");
        assert!(before.can_auto_install());
        let tmp = tempfile::tempdir().unwrap();
        let log = OpLog::create(tmp.path(), "deps-install").unwrap();
        let pkgs = before.packages();
        run_install(before.family(), &pkgs, &log, &mut |_, l| eprintln!("{l}"), &Cancel::new()).unwrap();
        let after = crate::deps::check();
        assert!(after.ready(), "still missing: {:?}", after.missing);
    }
}
