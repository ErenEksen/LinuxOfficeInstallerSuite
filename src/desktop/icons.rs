//! Launcher icons, extracted from the user's own installed EXEs with `wrestool` and
//! `icotool` (desktop-integration: Icons from the user's installation). If extraction
//! fails, a built-in generic document icon is used. No Microsoft icon ships with the app.

use std::io;
use std::path::{Path, PathBuf};

use crate::core::Cancel;
use crate::core::ledger::{Category, Ledger};
use crate::core::paths::Paths;
use crate::core::proc::{Cmd, run_checked};

/// Built-in fallback icon, per hicolor size.
pub const GENERIC: &[(u32, &[u8])] = &[
    (16, include_bytes!("../../assets/icons/hicolor/16x16/apps/msoffice-generic-document.png")),
    (32, include_bytes!("../../assets/icons/hicolor/32x32/apps/msoffice-generic-document.png")),
    (48, include_bytes!("../../assets/icons/hicolor/48x48/apps/msoffice-generic-document.png")),
    (64, include_bytes!("../../assets/icons/hicolor/64x64/apps/msoffice-generic-document.png")),
    (128, include_bytes!("../../assets/icons/hicolor/128x128/apps/msoffice-generic-document.png")),
    (256, include_bytes!("../../assets/icons/hicolor/256x256/apps/msoffice-generic-document.png")),
    (512, include_bytes!("../../assets/icons/hicolor/512x512/apps/msoffice-generic-document.png")),
];

/// The app's own icon, per hicolor size.
pub const APP_ICON: &[(u32, &[u8])] = &[
    (16, include_bytes!("../../assets/icons/hicolor/16x16/apps/linux-office-installer-suite.png")),
    (32, include_bytes!("../../assets/icons/hicolor/32x32/apps/linux-office-installer-suite.png")),
    (48, include_bytes!("../../assets/icons/hicolor/48x48/apps/linux-office-installer-suite.png")),
    (64, include_bytes!("../../assets/icons/hicolor/64x64/apps/linux-office-installer-suite.png")),
    (128, include_bytes!("../../assets/icons/hicolor/128x128/apps/linux-office-installer-suite.png")),
    (256, include_bytes!("../../assets/icons/hicolor/256x256/apps/linux-office-installer-suite.png")),
    (512, include_bytes!("../../assets/icons/hicolor/512x512/apps/linux-office-installer-suite.png")),
];

const SIZES: &[u32] = &[16, 22, 24, 32, 48, 64, 96, 128, 256, 512];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconSource {
    Extracted,
    Generic,
}

/// Parse icotool's output name `<stem>_<idx>_<w>x<h>x<bpp>.png` into (size, bpp).
fn parse_png_name(name: &str) -> Option<(u32, u32)> {
    let dims = name.strip_suffix(".png")?.rsplit('_').next()?;
    let mut it = dims.split('x').map(|n| n.parse::<u32>().ok());
    let (w, h, bpp) = (it.next()??, it.next()??, it.next()??);
    (w == h).then_some((w, bpp))
}

/// Order `wrestool` output names by their numeric resource id where possible.
fn resource_key(name: &str) -> (u64, String) {
    let id = name.split('_').nth(2).and_then(|n| n.parse::<u64>().ok()).unwrap_or(u64::MAX);
    (id, name.to_string())
}

/// Extract the first group icon of `exe` into PNGs. Returns the best PNG per size.
pub fn extract(exe: &Path, work: &Path, cancel: &Cancel) -> Option<Vec<(u32, PathBuf)>> {
    let _ = std::fs::remove_dir_all(work);
    let (ico_dir, png_dir) = (work.join("ico"), work.join("png"));
    std::fs::create_dir_all(&ico_dir).ok()?;
    std::fs::create_dir_all(&png_dir).ok()?;
    run_checked(
        &Cmd::new("wrestool").args(["-x", "-t", "14", "-o"]).arg(&ico_dir).arg(exe),
        None,
        &mut |_, _| {},
        cancel,
    )
    .ok()?;
    let mut icos: Vec<PathBuf> = std::fs::read_dir(&ico_dir).ok()?.flatten().map(|e| e.path()).collect();
    icos.sort_by_key(|p| resource_key(&p.file_name().unwrap_or_default().to_string_lossy()));
    let ico = icos.first()?;
    run_checked(&Cmd::new("icotool").args(["-x", "-o"]).arg(&png_dir).arg(ico), None, &mut |_, _| {}, cancel).ok()?;

    let mut best: Vec<(u32, u32, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(&png_dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((size, bpp)) = parse_png_name(&name) else { continue };
        if !SIZES.contains(&size) {
            continue;
        }
        match best.iter_mut().find(|b| b.0 == size) {
            Some(b) if b.1 >= bpp => {}
            Some(b) => *b = (size, bpp, e.path()),
            None => best.push((size, bpp, e.path())),
        }
    }
    best.sort();
    (!best.is_empty()).then(|| best.into_iter().map(|(s, _, p)| (s, p)).collect())
}

fn icon_path(paths: &Paths, size: u32, name: &str) -> PathBuf {
    paths.icons_hicolor().join(format!("{size}x{size}/apps/{name}.png"))
}

/// Write PNG bytes as hicolor icon `name` and record each file in the ledger.
pub fn install_pngs(
    paths: &Paths,
    ledger: &mut Ledger,
    category: Category,
    name: &str,
    pngs: &[(u32, Vec<u8>)],
) -> io::Result<()> {
    for (size, bytes) in pngs {
        let dest = icon_path(paths, *size, name);
        std::fs::create_dir_all(dest.parent().unwrap())?;
        ledger.record_file(&dest, category)?;
        std::fs::write(&dest, bytes)?;
    }
    Ok(())
}

/// Install the icon for one Office launcher as `msoffice-<id>` (extracted, else generic).
pub fn install_app_icon(
    paths: &Paths,
    ledger: &mut Ledger,
    icon_name: &str,
    exe: Option<&Path>,
    cancel: &Cancel,
) -> io::Result<IconSource> {
    let work = paths.cache().join(format!("work-icon-{icon_name}"));
    let extracted = exe.and_then(|e| extract(e, &work, cancel)).and_then(|list| {
        list.into_iter().map(|(s, p)| std::fs::read(p).ok().map(|b| (s, b))).collect::<Option<Vec<_>>>()
    });
    let _ = std::fs::remove_dir_all(&work);
    let (pngs, source) = match extracted {
        Some(p) if !p.is_empty() => (p, IconSource::Extracted),
        _ => (GENERIC.iter().map(|(s, b)| (*s, b.to_vec())).collect(), IconSource::Generic),
    };
    install_pngs(paths, ledger, Category::Office, icon_name, &pngs)?;
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_icotool_names() {
        assert_eq!(parse_png_name("notepad.exe_14_768_0_1_256x256x32.png"), Some((256, 32)));
        assert_eq!(parse_png_name("x_14_1_0_3_16x16x4.png"), Some((16, 4)));
        assert_eq!(parse_png_name("x_14_1_0_3_16x32x4.png"), None);
        assert!(resource_key("WINWORD.EXE_14_1_1033.ico") < resource_key("WINWORD.EXE_14_20_1033.ico"));
    }

    #[test]
    fn fallback_when_no_icon() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let mut ledger = Ledger::load(&paths.ledger_file());
        let not_an_exe = tmp.path().join("MSPUB.EXE");
        std::fs::write(&not_an_exe, b"no resources here").unwrap();
        let src = install_app_icon(&paths, &mut ledger, "msoffice-publisher", Some(&not_an_exe), &Cancel::new()).unwrap();
        assert_eq!(src, IconSource::Generic);
        let p = icon_path(&paths, 48, "msoffice-publisher");
        assert_eq!(std::fs::read(&p).unwrap(), GENERIC[2].1);
        assert_eq!(ledger.files(Category::Office).count(), GENERIC.len());
    }

    /// Extract from a real EXE in the test system's prefix: `cargo test -- --ignored live_icon`.
    #[test]
    #[ignore]
    fn live_icon_from_notepad() {
        let real = Paths::from_env();
        let exe = real.prefix().join("drive_c/windows/system32/notepad.exe");
        let tmp = tempfile::tempdir().unwrap();
        let list = extract(&exe, &tmp.path().join("w"), &Cancel::new()).expect("icons");
        eprintln!("{:?}", list.iter().map(|(s, _)| s).collect::<Vec<_>>());
        assert!(list.iter().any(|(s, _)| *s == 256) && list.iter().any(|(s, _)| *s == 16));
        let paths = Paths::for_home(tmp.path());
        let mut ledger = Ledger::load(&paths.ledger_file());
        let src = install_app_icon(&paths, &mut ledger, "msoffice-notepad", Some(&exe), &Cancel::new()).unwrap();
        assert_eq!(src, IconSource::Extracted);
        assert!(icon_path(&paths, 32, "msoffice-notepad").exists());
        let sppc = real.prefix().join("drive_c/windows/syswow64/sppc.dll");
        let src = install_app_icon(&paths, &mut ledger, "msoffice-none", Some(&sppc), &Cancel::new()).unwrap();
        assert_eq!(src, IconSource::Generic, "a DLL without icons falls back");
    }
}
