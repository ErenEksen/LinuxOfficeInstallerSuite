//! Required and optional sonames for the Soda runner, and the package that provides each
//! one per distro family (design D7).
//!
//! The Arch map is verified in the test container (every package exists in the repos and
//! together they let Soda run 32-bit and 64-bit programs). The Fedora (`.i686`) and
//! Debian/Ubuntu (`:i386`) maps were written from the package databases and are UNTESTED:
//! the UI labels those families "Untested" until they are verified on real machines.
//! `flake.nix` provides the same set on NixOS; a test keeps the two lists in step.

use super::os_release::Family;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bits {
    B64,
    B32,
}

impl Bits {
    pub fn label(self) -> &'static str {
        match self {
            Bits::B64 => "64-bit",
            Bits::B32 => "32-bit",
        }
    }
}

/// Package names for one family: (64-bit, 32-bit). Empty = not needed for that width.
type Pkgs = (&'static str, &'static str);

#[derive(Debug)]
pub struct Lib {
    pub soname: &'static str,
    pub need64: bool,
    pub need32: bool,
    pub arch: Pkgs,
    /// UNTESTED
    pub fedora: Pkgs,
    /// UNTESTED
    pub debian: Pkgs,
    /// nixpkgs attribute in `flake.nix` (`winePkgs`, both widths); empty for OPTIONAL.
    /// Only read by `flake_provides_every_required_package`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub nix: &'static str,
}

impl Lib {
    pub fn package(&self, family: Family, bits: Bits) -> Option<&'static str> {
        let pkgs = match family {
            Family::Arch => self.arch,
            Family::Fedora => self.fedora,
            Family::Debian => self.debian,
            Family::NixOS | Family::Unknown => return None,
        };
        let p = match bits {
            Bits::B64 => pkgs.0,
            Bits::B32 => pkgs.1,
        };
        (!p.is_empty()).then_some(p)
    }

    pub fn needs(&self, bits: Bits) -> bool {
        match bits {
            Bits::B64 => self.need64,
            Bits::B32 => self.need32,
        }
    }
}

const fn both(soname: &'static str, arch: Pkgs, fedora: Pkgs, debian: Pkgs, nix: &'static str) -> Lib {
    Lib { soname, need64: true, need32: true, arch, fedora, debian, nix }
}

/// Libraries Wine needs to start, draw windows and run Office.
pub const REQUIRED: &[Lib] = &[
    Lib {
        soname: "ld-linux.so.2",
        need64: false,
        need32: true,
        arch: ("", "lib32-glibc"),
        fedora: ("", "glibc.i686"),
        debian: ("", "libc6:i386"),
        nix: "glibc",
    },
    Lib {
        soname: "libunwind.so.8",
        need64: true,
        need32: false,
        arch: ("libunwind", ""),
        fedora: ("libunwind", ""),
        debian: ("libunwind8", ""),
        nix: "libunwind",
    },
    both("libfreetype.so.6", ("freetype2", "lib32-freetype2"), ("freetype", "freetype.i686"), ("libfreetype6", "libfreetype6:i386"), "freetype"),
    both("libfontconfig.so.1", ("fontconfig", "lib32-fontconfig"), ("fontconfig", "fontconfig.i686"), ("libfontconfig1", "libfontconfig1:i386"), "fontconfig"),
    both("libgnutls.so.30", ("gnutls", "lib32-gnutls"), ("gnutls", "gnutls.i686"), ("libgnutls30t64", "libgnutls30t64:i386"), "gnutls"),
    both("libX11.so.6", ("libx11", "lib32-libx11"), ("libX11", "libX11.i686"), ("libx11-6", "libx11-6:i386"), "libx11"),
    both("libXext.so.6", ("libxext", "lib32-libxext"), ("libXext", "libXext.i686"), ("libxext6", "libxext6:i386"), "libxext"),
    both("libXrandr.so.2", ("libxrandr", "lib32-libxrandr"), ("libXrandr", "libXrandr.i686"), ("libxrandr2", "libxrandr2:i386"), "libxrandr"),
    both("libXcursor.so.1", ("libxcursor", "lib32-libxcursor"), ("libXcursor", "libXcursor.i686"), ("libxcursor1", "libxcursor1:i386"), "libxcursor"),
    both("libXrender.so.1", ("libxrender", "lib32-libxrender"), ("libXrender", "libXrender.i686"), ("libxrender1", "libxrender1:i386"), "libxrender"),
    both("libXxf86vm.so.1", ("libxxf86vm", "lib32-libxxf86vm"), ("libXxf86vm", "libXxf86vm.i686"), ("libxxf86vm1", "libxxf86vm1:i386"), "libxxf86vm"),
    both("libxkbcommon.so.0", ("libxkbcommon", "lib32-libxkbcommon"), ("libxkbcommon", "libxkbcommon.i686"), ("libxkbcommon0", "libxkbcommon0:i386"), "libxkbcommon"),
    both("libwayland-client.so.0", ("wayland", "lib32-wayland"), ("libwayland-client", "libwayland-client.i686"), ("libwayland-client0", "libwayland-client0:i386"), "wayland"),
    both("libGL.so.1", ("libglvnd", "lib32-libglvnd"), ("libglvnd-glx", "libglvnd-glx.i686"), ("libgl1", "libgl1:i386"), "libglvnd"),
    both("libEGL.so.1", ("libglvnd", "lib32-libglvnd"), ("libglvnd-egl", "libglvnd-egl.i686"), ("libegl1", "libegl1:i386"), "libglvnd"),
    both("libvulkan.so.1", ("vulkan-icd-loader", "lib32-vulkan-icd-loader"), ("vulkan-loader", "vulkan-loader.i686"), ("libvulkan1", "libvulkan1:i386"), "vulkan-loader"),
    both("libpulse.so.0", ("libpulse", "lib32-libpulse"), ("pulseaudio-libs", "pulseaudio-libs.i686"), ("libpulse0", "libpulse0:i386"), "libpulseaudio"),
    both("libasound.so.2", ("alsa-lib", "lib32-alsa-lib"), ("alsa-lib", "alsa-lib.i686"), ("libasound2t64", "libasound2t64:i386"), "alsa-lib"),
    both("libudev.so.1", ("systemd-libs", "lib32-systemd"), ("systemd-libs", "systemd-libs.i686"), ("libudev1", "libudev1:i386"), "udev"),
    both("libdbus-1.so.3", ("dbus", "lib32-dbus"), ("dbus-libs", "dbus-libs.i686"), ("libdbus-1-3", "libdbus-1-3:i386"), "dbus"),
    both("libkrb5.so.3", ("krb5", "lib32-krb5"), ("krb5-libs", "krb5-libs.i686"), ("libkrb5-3", "libkrb5-3:i386"), "krb5"),
    both("libgmp.so.10", ("gmp", "lib32-gmp"), ("gmp", "gmp.i686"), ("libgmp10", "libgmp10:i386"), "gmp"),
    both("libgcrypt.so.20", ("libgcrypt", "lib32-libgcrypt"), ("libgcrypt", "libgcrypt.i686"), ("libgcrypt20", "libgcrypt20:i386"), "libgcrypt"),
    both("libcups.so.2", ("libcups", "lib32-libcups"), ("cups-libs", "cups-libs.i686"), ("libcups2t64", "libcups2t64:i386"), "cups"),
];

const fn opt(soname: &'static str, arch: &'static str, fedora: &'static str, debian: &'static str) -> Lib {
    Lib { soname, need64: true, need32: false, arch: (arch, ""), fedora: (fedora, ""), debian: (debian, ""), nix: "" }
}

/// Libraries Wine can use but Office does not need. Reported, never required or installed.
pub const OPTIONAL: &[Lib] = &[
    opt("libavcodec.so.60", "ffmpeg", "ffmpeg-free", "libavcodec60"),
    opt("libgstreamer-1.0.so.0", "gstreamer", "gstreamer1", "libgstreamer1.0-0"),
    opt("libsane.so.1", "sane", "sane-backends-libs", "libsane1"),
    opt("libgphoto2.so.6", "libgphoto2", "libgphoto2", "libgphoto2-6t64"),
    opt("libOpenCL.so.1", "ocl-icd", "ocl-icd", "ocl-icd-libopencl1"),
    opt("libpcap.so.1", "libpcap", "libpcap", "libpcap0.8t64"),
    opt("libopenxr_loader.so.1", "openxr", "openxr-libs", "libopenxr-loader1"),
    opt("libcapi20.so.3", "", "", "libcapi20-3t64"),
];

#[derive(Debug)]
pub struct Tool {
    pub name: &'static str,
    /// Package per family: arch, fedora (UNTESTED), debian (UNTESTED).
    pub packages: [&'static str; 3],
    /// nixpkgs attribute in `flake.nix` (`targetPkgs`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub nix: &'static str,
}

impl Tool {
    pub fn package(&self, family: Family) -> Option<&'static str> {
        match family {
            Family::Arch => Some(self.packages[0]),
            Family::Fedora => Some(self.packages[1]),
            Family::Debian => Some(self.packages[2]),
            Family::NixOS | Family::Unknown => None,
        }
    }
}

/// Helper tools the installer itself runs.
pub const TOOLS: &[Tool] = &[
    Tool { name: "cabextract", packages: ["cabextract", "cabextract", "cabextract"], nix: "cabextract" },
    Tool { name: "wrestool", packages: ["icoutils", "icoutils", "icoutils"], nix: "icoutils" },
    Tool { name: "icotool", packages: ["icoutils", "icoutils", "icoutils"], nix: "icoutils" },
];

pub const UNTESTED_NOTE: &str = "This package list is untested on this distribution family.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_set_covers_the_spec() {
        let names: Vec<_> = REQUIRED.iter().map(|l| l.soname).collect();
        assert!(names.contains(&"libunwind.so.8"));
        let ld = REQUIRED.iter().find(|l| l.soname == "ld-linux.so.2").unwrap();
        assert!(ld.need32 && !ld.need64);
        for l in REQUIRED {
            for fam in [Family::Arch, Family::Fedora, Family::Debian] {
                for bits in [Bits::B64, Bits::B32] {
                    assert_eq!(l.needs(bits), l.package(fam, bits).is_some(), "{} {fam:?} {bits:?}", l.soname);
                }
            }
            if l.need32 {
                assert!(l.package(Family::Fedora, Bits::B32).unwrap().ends_with(".i686"));
                assert!(l.package(Family::Debian, Bits::B32).unwrap().ends_with(":i386"));
            }
        }
        assert!(OPTIONAL.iter().all(|l| !l.need32));
    }

    /// The words of the Nix list that starts after `marker`, up to its closing `];`.
    fn nix_list<'a>(flake: &'a str, marker: &str) -> Vec<&'a str> {
        let start = flake.find(marker).unwrap_or_else(|| panic!("flake.nix has no `{marker}`"));
        let body = &flake[start + marker.len()..];
        let body = &body[..body.find("];").unwrap_or_else(|| panic!("`{marker}` list is not closed"))];
        body.lines().flat_map(|l| l.split('#').next().unwrap_or("").split_whitespace()).collect()
    }

    /// The flake's FHS environment must carry everything the check requires, or NixOS users
    /// see missing libraries inside `nix run`.
    #[test]
    fn flake_provides_every_required_package() {
        let flake = include_str!("../../flake.nix");
        let wine = nix_list(flake, "winePkgs =");
        for l in REQUIRED {
            assert!(wine.contains(&l.nix), "{} ({}) is missing from winePkgs in flake.nix", l.nix, l.soname);
        }
        let target = nix_list(flake, "targetPkgs =");
        for t in TOOLS {
            let attr = format!("p.{}", t.nix);
            assert!(target.contains(&attr.as_str()), "{attr} ({}) is missing from targetPkgs in flake.nix", t.name);
        }
    }

    /// Every Arch package in the maps exists in the configured repos. Needs pacman, so it
    /// only runs on request: `cargo test -- --ignored arch_packages_exist`.
    #[test]
    #[ignore]
    fn arch_packages_exist() {
        let mut pkgs: Vec<&str> = REQUIRED
            .iter()
            .chain(OPTIONAL)
            .flat_map(|l| [l.arch.0, l.arch.1])
            .chain(TOOLS.iter().map(|t| t.packages[0]))
            .filter(|p| !p.is_empty())
            .collect();
        pkgs.sort();
        pkgs.dedup();
        let out = std::process::Command::new("pacman").env("LC_ALL", "C").arg("-Si").args(&pkgs).output().unwrap();
        // pacman -Si exits non-zero and names the package if any one is not found.
        assert!(out.status.success(), "missing packages: {}", String::from_utf8_lossy(&out.stderr));
        let names = String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.starts_with("Name ")).count();
        assert!(names >= pkgs.len(), "{names} of {}", pkgs.len());
    }
}
