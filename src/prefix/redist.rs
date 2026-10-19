//! Microsoft redistributables, ported from the Bottles `office-runtime`, `gdiplus`, `msxml6`,
//! `riched20` and `msls31` recipes (design D4). Every archive is downloaded from its
//! original or archived public source at install time, checked against a pinned MD5 (from
//! the recipe) and SHA-256 (pinned during implementation), cached in `~/.msoffice/cache/`,
//! and unpacked with `cabextract`. Nothing here is bundled with the app.

use std::path::{Path, PathBuf};

use reqwest::blocking::Client;

use super::steps::StepError;
use super::wine::Wine;
use crate::core::Cancel;
use crate::core::download::{Checksum, download};
use crate::core::proc::{Cmd, OpLog, run_checked};

#[derive(Debug)]
pub struct Archive {
    /// File name in the cache.
    pub file: &'static str,
    pub url: &'static str,
    pub md5: &'static str,
    pub sha256: &'static str,
    pub size: u64,
}

impl Archive {
    pub fn checksums(&self) -> [Checksum; 2] {
        [Checksum::Md5(self.md5.into()), Checksum::Sha256(self.sha256.into())]
    }

    /// Download into `cache_dir` (or reuse a matching cached copy) and return the path.
    pub fn fetch(
        &self,
        client: &Client,
        cache_dir: &Path,
        progress: &mut dyn FnMut(u64, Option<u64>),
        log: &OpLog,
        cancel: &Cancel,
    ) -> Result<PathBuf, StepError> {
        let dest = cache_dir.join(self.file);
        log.line(&format!("fetching {} ({} bytes) from {}", self.file, self.size, self.url));
        download(client, self.url, &dest, &self.checksums(), progress, cancel).map_err(|e| match e {
            crate::core::download::DownloadError::Cancelled => StepError::Cancelled,
            other => StepError::msg(format!("{}: {other}", self.file)),
        })?;
        Ok(dest)
    }
}

/// Where an extracted file goes. Bottles' `win32` is `syswow64` in a 64-bit prefix and
/// `win64` is `system32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    Syswow64,
    System32,
}

#[derive(Debug)]
pub struct Extract {
    /// Member path inside the archive (a `cabextract -F` pattern).
    pub member: &'static str,
    /// For archives in archives (`.msi`, `.dl_`): the member to take from the extracted file.
    pub nested: Option<&'static str>,
    pub dest: Dest,
    /// Final file name in `dest`.
    pub name: &'static str,
}

#[derive(Debug)]
pub struct Package {
    /// Step id suffix and log label.
    pub id: &'static str,
    pub title: &'static str,
    pub archive: &'static Archive,
    pub extracts: &'static [Extract],
}

pub const WIN7SP1: Archive = Archive {
    file: "windows6.1-kb976932-x64.exe",
    url: "http://download.windowsupdate.com/msdownload/update/software/svpk/2011/02/windows6.1-kb976932-x64_74865ef2562006e51d7f9333b4a8d45b7a749dab.exe",
    md5: "28d3932f714bf71d78e75d36aa2e0fb8",
    sha256: "f4d1d418d91b1619688a482680ee032ffd2b65e420c6d2eaecf8aa3762aa64c8",
    size: 947_070_088,
};

pub const MSXML6: Archive = Archive {
    file: "msxml6-KB2957482-enu-amd64.exe",
    url: "https://download.microsoft.com/download/2/7/7/277681BE-4048-4A58-ABBA-259C465B1699/msxml6-KB2957482-enu-amd64.exe",
    md5: "7efa46a2a22a00b2df30c84d269caef3",
    sha256: "260cd870851ffc3c6d10b71691f134e20d8d03ac26073bb36951eacb7aa85897",
    size: 1_885_336,
};

pub const W2KSP4: Archive = Archive {
    file: "w2ksp4_en.exe",
    url: "https://proxy.usebottles.com/redistributable/dependencies/W2KSP4_EN.EXE",
    md5: "a4ef6c91d418418b287cefe31f958175",
    sha256: "167bb78d4adc957cc39fb4902517e1f32b1e62092353be5f8fb9ee647642de7e",
    size: 135_477_136,
};

pub const INSTMSIW: Archive = Archive {
    file: "InstMsiW.exe",
    url: "https://proxy.usebottles.com/redistributable/dependencies/InstMsiW.exe",
    md5: "53820efbc952107ee1a38be6cd5aa3f0",
    sha256: "4c3516c0b5c2b76b88209b22e3bf1cb82d8e2de7116125e97e128952372eed6b",
    size: 1_822_848,
};

pub const Q811493: Archive = Archive {
    file: "Q811493_W2K_SP4_X86_EN.exe",
    url: "https://web.archive.org/web/20150526022037/http://download.microsoft.com/download/8/d/c/8dc79965-dfbc-4b25-9546-e23bc4b791c6/Q811493_W2K_SP4_X86_EN.exe",
    md5: "e6fc7d6603cbacbcad1ec62d9b6f4018",
    sha256: "b5574b3516a724c2cba0d864162a3d1d684db1cf30de8db4b0e0ea6a1f6f1480",
    size: 5_459_984,
};

const fn ex(member: &'static str, dest: Dest, name: &'static str) -> Extract {
    Extract { member, nested: None, dest, name }
}

/// The recipe, in install order. The Win7 SP1 package is fetched once and shared.
pub const PACKAGES: &[Package] = &[
    Package {
        id: "win7sp1",
        title: "GDI+, licensing client and font embedding (Windows 7 SP1)",
        archive: &WIN7SP1,
        extracts: &[
            ex("x86_microsoft.windows.gdiplus_6595b64144ccf1df_1.1.7601.17514_none_72d18a4386696c80/gdiplus.dll", Dest::Syswow64, "gdiplus.dll"),
            ex("amd64_microsoft.windows.gdiplus_6595b64144ccf1df_1.1.7601.17514_none_2b24536c71ed437a/gdiplus.dll", Dest::System32, "gdiplus.dll"),
            ex("x86_microsoft-windows-security-spp-client_31bf3856ad364e35_6.1.7601.17514_none_98a45ba609e1caf5/sppc.dll", Dest::Syswow64, "sppc.dll"),
            ex("amd64_microsoft-windows-security-spp-client_31bf3856ad364e35_6.1.7601.17514_none_f4c2f729c23f3c2b/sppc.dll", Dest::System32, "sppc.dll"),
            ex("x86_microsoft-windows-font-embedding_31bf3856ad364e35_6.1.7601.17514_none_b7c78d327d35e10e/t2embed.dll", Dest::Syswow64, "t2embed.dll"),
            ex("amd64_microsoft-windows-font-embedding_31bf3856ad364e35_6.1.7601.17514_none_13e628b635935244/t2embed.dll", Dest::System32, "t2embed.dll"),
        ],
    },
    Package {
        id: "msxml6",
        title: "MSXML 6.0",
        archive: &MSXML6,
        extracts: &[
            Extract { member: "msxml6.msi", nested: Some("msxml6.dll.86F857F6_A743_463D_B2FE_98CB5F727E09"), dest: Dest::Syswow64, name: "msxml6.dll" },
            Extract { member: "msxml6.msi", nested: Some("msxml6r.dll.86F857F6_A743_463D_B2FE_98CB5F727E09"), dest: Dest::Syswow64, name: "msxml6r.dll" },
            Extract { member: "msxml6.msi", nested: Some("msxml6.dll.1ECC0691_D2EB_4A33_9CBF_5487E5CB17DB"), dest: Dest::System32, name: "msxml6.dll" },
            Extract { member: "msxml6.msi", nested: Some("msxml6r.dll.1ECC0691_D2EB_4A33_9CBF_5487E5CB17DB"), dest: Dest::System32, name: "msxml6r.dll" },
        ],
    },
    Package {
        id: "riched20",
        title: "RichEdit 2.0",
        archive: &W2KSP4,
        extracts: &[Extract { member: "i386/riched20.dl_", nested: Some("riched20.dll"), dest: Dest::Syswow64, name: "riched20.dll" }],
    },
    Package {
        id: "msls31",
        title: "Line Services",
        archive: &INSTMSIW,
        extracts: &[ex("msls31.dll", Dest::Syswow64, "msls31.dll")],
    },
    Package {
        id: "cmd",
        title: "Command prompt (Q811493)",
        archive: &Q811493,
        extracts: &[ex("cmd.exe", Dest::Syswow64, "cmd.exe")],
    },
];

/// DLL overrides from the recipe (wine-prefix-setup: DLL overrides). Keys with `*` apply
/// whatever path the DLL is loaded from, as in Bottles' office-runtime.
pub const OVERRIDES: &[(&str, &str)] = &[
    ("*cmd.exe", "native,builtin"),
    ("*gdiplus", "native"),
    ("*msxml6", "native,builtin"),
    ("msxml6r", "native,builtin"),
    ("*riched20", "native,builtin"),
    ("t2embed", "native,builtin"),
    ("msls31", "native,builtin"),
    ("sppc", "builtin"),
    ("winscard", "builtin"),
];

fn cabextract(archive: &Path, members: &[&str], out: &Path, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    std::fs::create_dir_all(out)?;
    let mut cmd = Cmd::new("cabextract").arg("-q").arg("-d").arg(out);
    for m in members {
        cmd = cmd.arg("-F").arg(*m);
    }
    run_checked(&cmd.arg(archive), Some(log), &mut |_, _| {}, cancel)?;
    Ok(())
}

/// Unpack `package` from its downloaded archive into the prefix.
pub fn install_package(
    package: &Package,
    archive_path: &Path,
    wine: &Wine,
    work: &Path,
    log: &OpLog,
    cancel: &Cancel,
) -> Result<(), StepError> {
    let _ = std::fs::remove_dir_all(work);
    let mut members: Vec<&str> = package.extracts.iter().map(|e| e.member).collect();
    members.dedup();
    let stage1 = work.join("1");
    cabextract(archive_path, &members, &stage1, log, cancel)?;

    for (i, e) in package.extracts.iter().enumerate() {
        let first = stage1.join(e.member);
        if !first.exists() {
            return Err(StepError::msg(format!("{}: {} was not found in {}", package.title, e.member, package.archive.file)));
        }
        let source = match e.nested {
            None => first,
            Some(inner) => {
                let stage2 = work.join(format!("2-{i}"));
                cabextract(&first, &[inner], &stage2, log, cancel)?;
                let p = stage2.join(inner);
                if !p.exists() {
                    return Err(StepError::msg(format!("{}: {inner} was not found in {}", package.title, e.member)));
                }
                p
            }
        };
        let dir = match e.dest {
            Dest::System32 => wine.system32(),
            Dest::Syswow64 => wine.syswow64(),
        };
        std::fs::create_dir_all(&dir)?;
        let target = dir.join(e.name);
        // Wine's builtin placeholder may be a symlink or read-only; replace it.
        let _ = std::fs::remove_file(&target);
        std::fs::copy(&source, &target)?;
        log.line(&format!("installed {}", target.display()));
    }
    let _ = std::fs::remove_dir_all(work);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_archive_has_pinned_hashes() {
        for p in PACKAGES {
            let a = p.archive;
            assert_eq!(a.md5.len(), 32, "{}", a.file);
            assert_eq!(a.sha256.len(), 64, "{} sha256 not pinned", a.file);
            assert!(a.md5.chars().chain(a.sha256.chars()).all(|c| c.is_ascii_hexdigit()));
            assert!(a.size > 0);
        }
    }

    #[test]
    fn covers_the_spec_components() {
        let names: Vec<&str> = PACKAGES.iter().flat_map(|p| p.extracts.iter().map(|e| e.name)).collect();
        for want in ["gdiplus.dll", "sppc.dll", "t2embed.dll", "msxml6.dll", "riched20.dll", "msls31.dll", "cmd.exe"] {
            assert!(names.contains(&want), "{want}");
        }
        // Both architectures for the Win7 SP1 parts and msxml6.
        for dll in ["gdiplus.dll", "sppc.dll", "t2embed.dll", "msxml6.dll"] {
            let dests: Vec<Dest> =
                PACKAGES.iter().flat_map(|p| p.extracts.iter()).filter(|e| e.name == dll).map(|e| e.dest).collect();
            assert!(dests.contains(&Dest::System32) && dests.contains(&Dest::Syswow64), "{dll}");
        }
    }
}
