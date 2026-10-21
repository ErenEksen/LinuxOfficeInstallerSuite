//! System dependency check: which required libraries and tools are missing, and which
//! packages provide them on this distro family.

pub mod install;
pub mod ldconfig;
pub mod loader;
pub mod maps;
pub mod os_release;
pub mod selfcheck;

use install::RepoStatus;
use ldconfig::LibSet;
use maps::{Bits, Lib};
use os_release::{Family, OsRelease};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    /// e.g. `libfreetype.so.6 (32-bit)` or `cabextract`
    pub name: String,
    pub package: Option<&'static str>,
}

impl Missing {
    /// One line of the list shown and copied: `name  →  package`.
    pub fn line(&self) -> String {
        match self.package {
            Some(p) => format!("{}  →  {p}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Why no library at all could be found. The check then fails instead of reporting every
/// library as missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undetected {
    /// Libraries live in the Nix store; prebuilt programs only see them in an FHS environment.
    NixOS,
    /// Same for the Guix store.
    Guix,
    /// musl instead of glibc: the Soda runner cannot run at all.
    Musl,
    /// No loader cache and nothing in the standard directories.
    Other,
}

const NIXOS_HELP: &str = "NixOS keeps libraries in the Nix store, where prebuilt programs such as the Wine runner \
cannot find them, even when the packages are installed. Start this app from the project's flake, which runs it in an \
FHS environment with every library listed below:\n\n    nix run github:ErenEksen/LinuxOfficeInstallerSuite\n\n\
With programs.nix-ld instead, add these libraries to programs.nix-ld.libraries. Then choose Check again.";

/// Shown on NixOS when libraries were found, i.e. inside the flake's FHS environment.
const NIXOS_FHS_HELP: &str = "Running on NixOS. The project's flake provides every library and tool listed here; \
anything still missing means the flake is out of date, so please report it.";

impl Undetected {
    pub fn title(self) -> &'static str {
        match self {
            Undetected::NixOS => "NixOS needs an FHS environment",
            Undetected::Guix => "Guix needs an FHS environment",
            Undetected::Musl => "This system is not supported",
            Undetected::Other => "Could not detect the installed libraries",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Undetected::NixOS => NIXOS_HELP,
            Undetected::Guix => {
                "Guix keeps libraries in the store, where prebuilt programs such as the Wine runner cannot find \
                 them. Start this app inside `guix shell --container --emulate-fhs` with the 64-bit and 32-bit \
                 libraries listed below, then choose Check again."
            }
            Undetected::Musl => {
                "This system uses the musl C library. The Wine runner is built for glibc and cannot run here."
            }
            Undetected::Other => {
                "No shared libraries were found: there is no loader cache (ldconfig) and the standard library \
                 directories hold none of the libraries below. Install them with your package manager, then \
                 choose Check again."
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DepReport {
    pub os: OsRelease,
    pub missing: Vec<Missing>,
    pub optional_missing: Vec<Missing>,
    pub repo: RepoStatus,
    /// Set when no library could be found at all; `missing` then lists everything required.
    pub undetected: Option<Undetected>,
}

impl DepReport {
    pub fn ready(&self) -> bool {
        self.missing.is_empty() && self.undetected.is_none()
    }

    pub fn family(&self) -> Family {
        self.os.family
    }

    /// Packages that would install every missing required item (deduplicated, in order).
    pub fn packages(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for p in self.missing.iter().filter_map(|m| m.package) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    }

    /// Items without a known package (unknown distro).
    pub fn missing_names(&self) -> Vec<String> {
        self.missing.iter().map(|m| m.name.clone()).collect()
    }

    pub fn can_auto_install(&self) -> bool {
        self.undetected.is_none()
            && self.family().support() != os_release::SupportLevel::Manual
            && matches!(self.repo, RepoStatus::Enabled)
            && !self.packages().is_empty()
    }

    /// Explanation under the distro name; NixOS gets its own even when libraries were found
    /// (inside the flake's FHS environment there is still no package map).
    pub fn support_explanation(&self) -> &'static str {
        match self.family() {
            Family::NixOS => NIXOS_FHS_HELP,
            f => f.support().explanation(),
        }
    }

    /// The list behind the Copy button: every missing (or, when undetected, required) item,
    /// then the install command when there is one.
    pub fn copy_text(&self) -> String {
        if self.missing.is_empty() {
            return String::new();
        }
        let what = if self.undetected.is_some() { "Required" } else { "Missing" };
        let mut out = format!("{what} on {} ({} items):\n", self.os.pretty_name, self.missing.len());
        for m in &self.missing {
            out.push_str(&m.line());
            out.push('\n');
        }
        if let Some(cmd) = install::install_command(self.family(), &self.packages()) {
            out.push('\n');
            out.push_str(&install::copyable_command(&cmd));
            out.push('\n');
        }
        out
    }
}

fn missing_libs(libs: &[Lib], set: &LibSet, family: Family) -> Vec<Missing> {
    let mut out = Vec::new();
    for bits in [Bits::B64, Bits::B32] {
        let present = match bits {
            Bits::B64 => &set.lib64,
            Bits::B32 => &set.lib32,
        };
        for lib in libs.iter().filter(|l| l.needs(bits)) {
            if !present.contains(lib.soname) {
                out.push(Missing {
                    name: format!("{} ({})", lib.soname, bits.label()),
                    package: lib.package(family, bits),
                });
            }
        }
    }
    out
}

/// Why `libs` holds nothing at all, if it does.
fn undetected(os: &OsRelease, libs: &LibSet, musl: bool) -> Option<Undetected> {
    if !libs.is_empty() {
        return None;
    }
    Some(if os.family == Family::NixOS {
        Undetected::NixOS
    } else if os.id == "guix" {
        Undetected::Guix
    } else if musl {
        Undetected::Musl
    } else {
        Undetected::Other
    })
}

/// Pure evaluation, so it can be tested with fixtures. `musl`: a musl loader is installed.
pub fn evaluate(os: OsRelease, libs: &LibSet, has_tool: impl Fn(&str) -> bool, repo: RepoStatus, musl: bool) -> DepReport {
    let family = os.family;
    let undetected = undetected(&os, libs, musl);
    let mut missing = missing_libs(maps::REQUIRED, libs, family);
    for tool in maps::TOOLS {
        if !has_tool(tool.name) {
            missing.push(Missing { name: tool.name.into(), package: tool.package(family) });
        }
    }
    let optional_missing = missing_libs(maps::OPTIONAL, libs, family);
    DepReport { os, missing, optional_missing, repo, undetected }
}

/// Sonames to look up: everything in the maps, plus glibc itself so a system that has none
/// of them still counts as detected.
fn wanted() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = maps::REQUIRED.iter().chain(maps::OPTIONAL).map(|l| l.soname).collect();
    v.push("libc.so.6");
    v
}

fn has_musl_loader() -> bool {
    std::fs::read_dir("/lib")
        .map(|d| d.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().starts_with("ld-musl-")))
        .unwrap_or(false)
}

/// Check the running system.
pub fn check() -> DepReport {
    let os = os_release::detect();
    let repo = install::check_32bit_repo(os.family);
    let libs = loader::scan(&wanted());
    let report = evaluate(os, &libs, ldconfig::has_tool, repo, has_musl_loader());
    if let Some(u) = report.undetected {
        log::warn!("no libraries found ({u:?}); checked LD_LIBRARY_PATH, ldconfig -p, ld.so.conf and {:?}", loader::DEFAULT_DIRS);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: &str = include_str!("../../tests/fixtures/ldconfig-after-lib32.txt");
    const BEFORE: &str = include_str!("../../tests/fixtures/ldconfig-before-lib32.txt");

    fn arch() -> OsRelease {
        os_release::parse("ID=arch\n")
    }

    #[test]
    fn container_after_lib32_is_ready() {
        let r = evaluate(arch(), &ldconfig::parse(AFTER), |_| true, RepoStatus::Enabled, false);
        assert!(r.ready(), "{:?}", r.missing);
    }

    #[test]
    fn before_lib32_reports_32_bit_libs_with_packages() {
        let r = evaluate(arch(), &ldconfig::parse(BEFORE), |t| t != "cabextract", RepoStatus::Enabled, false);
        assert!(!r.ready());
        assert!(r.missing.iter().any(|m| m.name == "ld-linux.so.2 (32-bit)" && m.package == Some("lib32-glibc")));
        assert!(r.missing.iter().all(|m| !m.name.ends_with("(64-bit)")));
        let pkgs = r.packages();
        assert!(pkgs.contains(&"cabextract") && pkgs.contains(&"lib32-libglvnd"));
        assert_eq!(pkgs.iter().filter(|p| **p == "lib32-libglvnd").count(), 1, "deduplicated");
        assert!(r.can_auto_install());
    }

    #[test]
    fn unknown_family_lists_names_only() {
        let os = os_release::parse("ID=alpine\n");
        let r = evaluate(os, &ldconfig::parse(BEFORE), |_| false, RepoStatus::Enabled, false);
        assert!(r.packages().is_empty());
        assert!(!r.can_auto_install());
        assert!(r.missing_names().contains(&"wrestool".to_string()));
    }

    #[test]
    fn disabled_repo_blocks_auto_install() {
        let r = evaluate(
            arch(),
            &ldconfig::parse(BEFORE),
            |_| true,
            RepoStatus::Disabled { instructions: "x".into() },
            false,
        );
        assert!(!r.can_auto_install());
    }

    #[test]
    fn empty_scan_is_undetected_not_missing() {
        let empty = LibSet::default();
        let nixos = os_release::parse("ID=nixos\nPRETTY_NAME=\"NixOS 25.05\"\n");
        let r = evaluate(nixos, &empty, |_| true, RepoStatus::Enabled, false);
        assert_eq!(r.undetected, Some(Undetected::NixOS));
        assert!(Undetected::NixOS.message().contains("nix run github:ErenEksen/LinuxOfficeInstallerSuite"));
        assert!(!r.ready() && !r.can_auto_install());
        // Everything required is still listed, so it can be copied.
        assert_eq!(r.missing.len(), 46);
        let copy = r.copy_text();
        assert!(copy.starts_with("Required on NixOS 25.05 (46 items):\n"), "{copy}");
        assert!(copy.contains("\nld-linux.so.2 (32-bit)\n") && !copy.contains("sudo"));
        let guix = os_release::parse("ID=guix\n");
        assert_eq!(evaluate(guix, &empty, |_| true, RepoStatus::Enabled, false).undetected, Some(Undetected::Guix));
        let alpine = os_release::parse("ID=alpine\n");
        assert_eq!(evaluate(alpine.clone(), &empty, |_| true, RepoStatus::Enabled, true).undetected, Some(Undetected::Musl));
        assert_eq!(evaluate(alpine, &empty, |_| true, RepoStatus::Enabled, false).undetected, Some(Undetected::Other));
        // Arch with nothing found fails too instead of offering to install 49 packages.
        let r = evaluate(arch(), &empty, |_| false, RepoStatus::Enabled, false);
        assert_eq!(r.undetected, Some(Undetected::Other));
        assert!(!r.can_auto_install());
    }

    #[test]
    fn nixos_with_libraries_reports_what_is_missing() {
        // Inside the flake's buildFHSEnv the libraries are found; NixOS still has no package map.
        let nixos = os_release::parse("ID=nixos\n");
        let r = evaluate(nixos, &ldconfig::parse(AFTER), |t| t != "cabextract", RepoStatus::Enabled, false);
        assert_eq!(r.undetected, None);
        assert_eq!(r.missing_names(), ["cabextract"]);
        assert!(!r.can_auto_install());
        assert!(r.support_explanation().contains("flake"));
    }

    /// Prints what the running system reports: `cargo test -- --ignored --nocapture live_check`.
    #[test]
    #[ignore]
    fn live_check() {
        let r = check();
        eprintln!("{} {:?} undetected={:?}\n{}", r.os.pretty_name, r.family(), r.undetected, r.copy_text());
        assert!(r.undetected.is_none());
    }

    #[test]
    fn copy_text_lists_items_and_command() {
        let r = evaluate(arch(), &ldconfig::parse(AFTER), |t| t != "cabextract" && t != "icotool", RepoStatus::Enabled, false);
        assert_eq!(
            r.copy_text(),
            "Missing on arch (2 items):\ncabextract  →  cabextract\nicotool  →  icoutils\n\n\
             sudo pacman -S --needed --noconfirm cabextract icoutils\n"
        );
        let ok = evaluate(arch(), &ldconfig::parse(AFTER), |_| true, RepoStatus::Enabled, false);
        assert_eq!(ok.copy_text(), "");
    }
}
