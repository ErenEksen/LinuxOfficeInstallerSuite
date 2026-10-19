//! Core fonts (the Bottles list, from the corefonts archives winetricks also uses) and
//! Selawik with Segoe UI substitutes (wine-prefix-setup: Fonts).

use std::io::Read;
use std::path::Path;

use super::redist::Archive;
use super::steps::StepError;
use super::wine::{RegData, Wine};
use crate::core::Cancel;
use crate::core::proc::{Cmd, OpLog, run_checked};

#[derive(Debug)]
pub struct FontPack {
    pub archive: Archive,
    /// (file name inside the archive, name registered in the Fonts key)
    pub fonts: &'static [(&'static str, &'static str)],
}

macro_rules! pack {
    ($file:literal, $url:literal, $md5:literal, $sha:literal, $size:literal, [$(($f:literal, $n:literal)),* $(,)?]) => {
        FontPack {
            archive: Archive { file: $file, url: $url, md5: $md5, sha256: $sha, size: $size },
            fonts: &[$(($f, $n)),*],
        }
    };
}

pub const CORE_FONTS: &[FontPack] = &[
    pack!("andale32.exe", "https://mirrors.kernel.org/gentoo/distfiles/7d/andale32.exe",
        "cbdc2fdd7d2ed0832795e86a8b9ee19a", "0524fe42951adc3a7eb870e32f0920313c71f170c859b5f770d82b4ee111e970", 198_384,
        [("AndaleMo.TTF", "Andale Mono")]),
    pack!("arial32.exe", "https://downloads.sourceforge.net/project/corefonts/the%20fonts/final/arial32.exe",
        "9637df0e91703179f0723ec095a36cb5", "85297a4d146e9c87ac6f74822734bdee5f4b2a722d7eaa584b7f2cbf76f478f6", 554_208,
        [("Arial.TTF", "Arial"), ("Arialbd.TTF", "Arial Bold"), ("Ariali.TTF", "Arial Italic"), ("Arialbi.TTF", "Arial Bold Italic")]),
    pack!("arialb32.exe", "https://mirrors.kernel.org/gentoo/distfiles/5d/arialb32.exe",
        "c9089ae0c3b3d0d8c4b0a95979bb9ff0", "a425f0ffb6a1a5ede5b979ed6177f4f4f4fdef6ae7c302a7b7720ef332fec0a8", 168_176,
        [("AriBlk.TTF", "Arial Black")]),
    pack!("comic32.exe", "https://mirrors.kernel.org/gentoo/distfiles/52/comic32.exe",
        "2b30de40bb5e803a0452c7715fc835d1", "9c6df3feefde26d4e41d4a4fe5db2a89f9123a772594d7f59afd062625cd204e", 246_008,
        [("Comic.TTF", "Comic Sans MS"), ("Comicbd.TTF", "Comic Sans MS Bold")]),
    pack!("courie32.exe", "https://mirrors.kernel.org/gentoo/distfiles/1b/courie32.exe",
        "4e412c772294403ab62fb2d247d85c60", "bb511d861655dde879ae552eb86b134d6fae67cb58502e6ff73ec5d9151f3384", 646_368,
        [("cour.ttf", "Courier New"), ("courbd.ttf", "Courier New Bold"), ("couri.ttf", "Courier New Italic"), ("courbi.ttf", "Courier New Bold Italic")]),
    pack!("georgi32.exe", "https://mirrors.kernel.org/gentoo/distfiles/f0/georgi32.exe",
        "4d90016026e2da447593b41a8d8fa8bd", "2c2c7dcda6606ea5cf08918fb7cd3f3359e9e84338dc690013f20cd42e930301", 392_440,
        [("Georgia.TTF", "Georgia"), ("Georgiab.TTF", "Georgia Bold"), ("Georgiai.TTF", "Georgia Italic"), ("Georgiaz.TTF", "Georgia Bold Italic")]),
    pack!("impact32.exe", "https://mirrors.kernel.org/gentoo/distfiles/10/impact32.exe",
        "7907c7dd6684e9bade91cff82683d9d7", "6061ef3b7401d9642f5dfdb5f2b376aa14663f6275e60a51207ad4facf2fccfb", 173_288,
        [("Impact.TTF", "Impact")]),
    pack!("times32.exe", "https://downloads.sourceforge.net/project/corefonts/the%20fonts/final/times32.exe",
        "ed39c8ef91b9fb80f76f702568291bd5", "db56595ec6ef5d3de5c24994f001f03b2a13e37cee27bc25c58f6f43e8f807ab", 661_728,
        [("Times.TTF", "Times New Roman"), ("Timesbd.TTF", "Times New Roman Bold"), ("Timesi.TTF", "Times New Roman Italic"), ("Timesbi.TTF", "Times New Roman Bold Italic")]),
    pack!("trebuc32.exe", "https://downloads.sourceforge.net/project/corefonts/the%20fonts/final/trebuc32.exe",
        "0d7ea16cac6261f8513a061fbfcdb2b5", "5a690d9bb8510be1b8b4fe49f1f2319651fe51bbe54775ddddd8ef0bd07fdac9", 357_200,
        [("trebuc.ttf", "Trebuchet MS"), ("Trebucbd.ttf", "Trebuchet MS Bold"), ("trebucit.ttf", "Trebuchet MS Italic"), ("trebucbi.ttf", "Trebuchet MS Bold Italic")]),
    pack!("verdan32.exe", "https://downloads.sourceforge.net/project/corefonts/the%20fonts/final/verdan32.exe",
        "12d2a75f8156e10607be1eaa8e8ef120", "c1cb61255e363166794e47664e2f21af8e3a26cb6346eb8d2ae2fa85dd5aad96", 351_992,
        [("Verdana.TTF", "Verdana"), ("Verdanab.TTF", "Verdana Bold"), ("Verdanai.TTF", "Verdana Italic"), ("Verdanaz.TTF", "Verdana Bold Italic")]),
    pack!("webdin32.exe", "https://mirrors.kernel.org/gentoo/distfiles/95/webdin32.exe",
        "230a1d13a365b22815f502eb24d9149b", "64595b5abc1080fba8610c5c34fab5863408e806aafe84653ca8575bed17d75a", 185_072,
        [("Webdings.TTF", "Webdings")]),
];

pub const SELAWIK: FontPack = pack!(
    "Selawik_Release.zip",
    "https://github.com/microsoft/Selawik/releases/download/1.01/Selawik_Release.zip",
    "dc596dc54a46b7779991d7a6c19cc705",
    "3f62c51e05e3b5a1e6241cf92a371f0be2ea1183aa87b30718bbd40832a8d423",
    530_652,
    [
        ("selawk.ttf", "Selawik"),
        ("selawkb.ttf", "Selawik Bold"),
        ("selawkl.ttf", "Selawik Light"),
        ("selawksb.ttf", "Selawik Semibold"),
        ("selawksl.ttf", "Selawik Semilight"),
    ]
);

/// Segoe UI → Selawik, as in the Bottles selawik recipe.
pub const SEGOE_SUBSTITUTES: &[(&str, &str)] = &[
    ("Segoe UI", "Selawik"),
    ("Segoe UI Bold", "Selawik"),
    ("Segoe UI Light", "Selawik Light"),
    ("Segoe UI Semibold", "Selawik Semibold"),
];

pub const FONTS_KEY: &str = r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows NT\CurrentVersion\Fonts";
pub const SUBST_KEYS: [&str; 2] = [
    r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows NT\CurrentVersion\FontSubstitutes",
    r"HKEY_LOCAL_MACHINE\Software\Wow6432Node\Microsoft\Windows NT\CurrentVersion\FontSubstitutes",
];
pub const REPLACEMENTS_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Fonts\Replacements";

/// Registry values that register `pack`'s fonts.
pub fn registrations(pack: &FontPack) -> Vec<(String, String, RegData)> {
    pack.fonts
        .iter()
        .map(|(file, name)| (FONTS_KEY.to_string(), format!("{name} (TrueType)"), RegData::Sz((*file).to_string())))
        .collect()
}

/// FontSubstitutes in both hives plus Wine's own replacement table.
pub fn substitutes() -> Vec<(String, String, RegData)> {
    let mut v = Vec::new();
    for key in SUBST_KEYS {
        for (from, to) in SEGOE_SUBSTITUTES {
            v.push((key.to_string(), (*from).to_string(), RegData::Sz((*to).to_string())));
        }
    }
    for (from, to) in SEGOE_SUBSTITUTES {
        v.push((REPLACEMENTS_KEY.to_string(), (*from).to_string(), RegData::Sz((*to).to_string())));
    }
    v
}

/// Unpack a corefonts `.exe` (a cabinet) into the prefix's Fonts folder.
pub fn install_cab_pack(pack: &FontPack, archive: &Path, wine: &Wine, work: &Path, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work)?;
    let mut cmd = Cmd::new("cabextract").arg("-q").arg("-d").arg(work);
    for (file, _) in pack.fonts {
        cmd = cmd.arg("-F").arg(*file);
    }
    run_checked(&cmd.arg(archive), Some(log), &mut |_, _| {}, cancel)?;
    copy_fonts(pack, |file| std::fs::read(work.join(file)).ok(), wine, log)?;
    let _ = std::fs::remove_dir_all(work);
    Ok(())
}

/// Unpack the Selawik zip into the prefix's Fonts folder.
pub fn install_zip_pack(pack: &FontPack, archive: &Path, wine: &Wine, log: &OpLog) -> Result<(), StepError> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| StepError::msg(format!("{}: {e}", pack.archive.file)))?;
    copy_fonts(
        pack,
        |name| {
            let mut entry = zip.by_name(name).ok()?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).ok()?;
            Some(buf)
        },
        wine,
        log,
    )
}

fn copy_fonts(
    pack: &FontPack,
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
    wine: &Wine,
    log: &OpLog,
) -> Result<(), StepError> {
    let dir = wine.fonts_dir();
    std::fs::create_dir_all(&dir)?;
    for (file, _) in pack.fonts {
        let bytes = read(file)
            .ok_or_else(|| StepError::msg(format!("{file} was not found in {}", pack.archive.file)))?;
        std::fs::write(dir.join(file), bytes)?;
        log.line(&format!("installed font {file}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_pinned_and_complete() {
        for p in CORE_FONTS.iter().chain(std::iter::once(&SELAWIK)) {
            assert_eq!(p.archive.md5.len(), 32);
            assert_eq!(p.archive.sha256.len(), 64);
            assert!(!p.fonts.is_empty());
        }
        assert_eq!(CORE_FONTS.len(), 11, "mirrors the Bottles list");
    }

    #[test]
    fn substitutes_cover_both_hives() {
        let s = substitutes();
        for key in SUBST_KEYS {
            assert!(s.iter().any(|(k, n, d)| k == key && n == "Segoe UI" && *d == RegData::Sz("Selawik".into())));
        }
        let r = registrations(&SELAWIK);
        assert_eq!(r[0].1, "Selawik (TrueType)");
    }
}
