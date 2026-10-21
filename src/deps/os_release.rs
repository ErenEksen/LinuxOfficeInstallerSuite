//! Distro family detection from `/etc/os-release` (system-dependencies: Distro family detection).

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Arch,
    Fedora,
    Debian,
    /// No package map: libraries live in the Nix store, outside the loader's search path.
    NixOS,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportLevel {
    /// Automatic install is tested.
    Supported,
    /// A package map exists but has not been verified.
    Untested,
    /// No package map: the user installs the listed libraries by hand.
    Manual,
}

impl Family {
    pub fn support(self) -> SupportLevel {
        match self {
            Family::Arch => SupportLevel::Supported,
            Family::Fedora | Family::Debian => SupportLevel::Untested,
            Family::NixOS | Family::Unknown => SupportLevel::Manual,
        }
    }

    fn from_id(id: &str) -> Option<Family> {
        match id {
            "arch" => Some(Family::Arch),
            "fedora" => Some(Family::Fedora),
            "debian" | "ubuntu" => Some(Family::Debian),
            "nixos" => Some(Family::NixOS),
            _ => None,
        }
    }
}

impl SupportLevel {
    pub fn label(self) -> &'static str {
        match self {
            SupportLevel::Supported => "Supported",
            SupportLevel::Untested => "Untested",
            SupportLevel::Manual => "Manual",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            SupportLevel::Supported => "Automatic package installation is tested on this distribution family.",
            SupportLevel::Untested => {
                "Automatic package installation has not been verified on this distribution family yet. \
                 The package list may be incomplete; the check after installing reports anything still missing."
            }
            SupportLevel::Manual => {
                "This distribution is not recognised. Install the missing libraries listed below with your \
                 package manager, then choose Check again."
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub id_like: Vec<String>,
    pub pretty_name: String,
    pub family: Family,
}

/// Parse os-release text. Values may be quoted with `"` or `'`.
pub fn parse(text: &str) -> OsRelease {
    let mut id = String::new();
    let mut id_like = Vec::new();
    let mut pretty_name = String::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = unquote(value.trim());
        match key.trim() {
            "ID" => id = value.to_ascii_lowercase(),
            "ID_LIKE" => id_like = value.split_whitespace().map(|s| s.to_ascii_lowercase()).collect(),
            "PRETTY_NAME" => pretty_name = value,
            _ => {}
        }
    }
    let family = std::iter::once(&id)
        .chain(&id_like)
        .find_map(|i| Family::from_id(i))
        .unwrap_or(Family::Unknown);
    if pretty_name.is_empty() {
        pretty_name = if id.is_empty() { "Linux".into() } else { id.clone() };
    }
    OsRelease { id, id_like, pretty_name, family }
}

fn unquote(v: &str) -> String {
    let v = v
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(v);
    v.replace("\\\"", "\"").replace("\\$", "$").replace("\\\\", "\\")
}

/// Read `/etc/os-release`, falling back to `/usr/lib/os-release` as the spec allows.
pub fn detect() -> OsRelease {
    detect_from(&[Path::new("/etc/os-release"), Path::new("/usr/lib/os-release")])
}

pub fn detect_from(candidates: &[&Path]) -> OsRelease {
    candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse(&t))
        .unwrap_or_else(|| parse(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &[(&str, &str, Family)] = &[
        ("Arch", "NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n", Family::Arch),
        ("CachyOS", "NAME=\"CachyOS Linux\"\nPRETTY_NAME=\"CachyOS\"\nID=cachyos\nID_LIKE=arch\n", Family::Arch),
        ("Manjaro", "NAME=\"Manjaro Linux\"\nID=manjaro\nID_LIKE=arch\nPRETTY_NAME=\"Manjaro Linux\"\n", Family::Arch),
        ("Fedora", "NAME=\"Fedora Linux\"\nVERSION_ID=42\nID=fedora\nPRETTY_NAME=\"Fedora Linux 42 (Workstation Edition)\"\n", Family::Fedora),
        ("Nobara", "NAME=\"Nobara Linux\"\nID=nobara\nID_LIKE=\"rhel centos fedora\"\nPRETTY_NAME=\"Nobara Linux 42 (KDE Plasma)\"\n", Family::Fedora),
        ("Debian", "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nID=debian\n", Family::Debian),
        ("Ubuntu", "PRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nNAME=\"Ubuntu\"\nID=ubuntu\nID_LIKE=debian\n", Family::Debian),
        ("Mint", "NAME=\"Linux Mint\"\nID=linuxmint\nID_LIKE=\"ubuntu debian\"\nPRETTY_NAME=\"Linux Mint 22.2\"\n", Family::Debian),
        ("openSUSE", "NAME=\"openSUSE Tumbleweed\"\nID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\nPRETTY_NAME=\"openSUSE Tumbleweed\"\n", Family::Unknown),
        ("NixOS", "ANSI_COLOR=\"0;38;2;126;186;228\"\nID=nixos\nNAME=NixOS\nPRETTY_NAME=\"NixOS 25.05 (Warbler)\"\n", Family::NixOS),
        ("Alpine", "NAME=\"Alpine Linux\"\nID=alpine\nVERSION_ID=3.22.1\nPRETTY_NAME=\"Alpine Linux v3.22\"\n", Family::Unknown),
    ];

    #[test]
    fn classifies_every_fixture() {
        for (name, text, family) in FIXTURES {
            let r = parse(text);
            assert_eq!(r.family, *family, "{name}");
            assert!(!r.pretty_name.is_empty());
        }
    }

    #[test]
    fn support_levels() {
        assert_eq!(Family::Arch.support().label(), "Supported");
        assert_eq!(Family::Fedora.support().label(), "Untested");
        assert_eq!(Family::Debian.support().label(), "Untested");
        assert_eq!(Family::NixOS.support().label(), "Manual");
        assert_eq!(Family::Unknown.support().label(), "Manual");
    }

    #[test]
    fn parses_quotes_and_missing_file() {
        let r = parse("ID='cachyos'\nID_LIKE=\"arch\"\nPRETTY_NAME=\"A \\\"quoted\\\" name\"\n# comment\n");
        assert_eq!(r.id, "cachyos");
        assert_eq!(r.pretty_name, "A \"quoted\" name");
        let r = detect_from(&[Path::new("/nonexistent/os-release")]);
        assert_eq!(r.family, Family::Unknown);
    }
}
