//! System dependency check: which required libraries and tools are missing, and which
//! packages provide them on this distro family.

pub mod install;
pub mod ldconfig;
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

#[derive(Clone, Debug)]
pub struct DepReport {
    pub os: OsRelease,
    pub missing: Vec<Missing>,
    pub optional_missing: Vec<Missing>,
    pub repo: RepoStatus,
}

impl DepReport {
    pub fn ready(&self) -> bool {
        self.missing.is_empty()
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
        self.family() != Family::Unknown && matches!(self.repo, RepoStatus::Enabled) && !self.packages().is_empty()
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

/// Pure evaluation, so it can be tested with fixtures.
pub fn evaluate(os: OsRelease, libs: &LibSet, has_tool: impl Fn(&str) -> bool, repo: RepoStatus) -> DepReport {
    let family = os.family;
    let mut missing = missing_libs(maps::REQUIRED, libs, family);
    for tool in maps::TOOLS {
        if !has_tool(tool.name) {
            missing.push(Missing { name: tool.name.into(), package: tool.package(family) });
        }
    }
    let optional_missing = missing_libs(maps::OPTIONAL, libs, family);
    DepReport { os, missing, optional_missing, repo }
}

/// Check the running system.
pub fn check() -> DepReport {
    let os = os_release::detect();
    let repo = install::check_32bit_repo(os.family);
    evaluate(os, &ldconfig::scan(), ldconfig::has_tool, repo)
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
        let r = evaluate(arch(), &ldconfig::parse(AFTER), |_| true, RepoStatus::Enabled);
        assert!(r.ready(), "{:?}", r.missing);
    }

    #[test]
    fn before_lib32_reports_32_bit_libs_with_packages() {
        let r = evaluate(arch(), &ldconfig::parse(BEFORE), |t| t != "cabextract", RepoStatus::Enabled);
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
        let r = evaluate(os, &ldconfig::parse(BEFORE), |_| false, RepoStatus::Enabled);
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
        );
        assert!(!r.can_auto_install());
    }
}
