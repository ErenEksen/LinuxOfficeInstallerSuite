//! Graphics settings from the Bottles Microsoft 365 recipe: DXVK, the OpenGL wined3d
//! renderer, the Direct3D/Direct2D values and the Wayland driver (wine-prefix-setup).

use std::io::Read;
use std::path::Path;

use super::redist::Archive;
use super::steps::StepError;
use super::wine::{RegData, Wine};

/// DXVK pinned to the newest release that works in a Soda 11.0-32 prefix (tested on
/// 2026-10-19: D3D11 device creation on Vulkan; see design.md).
pub const DXVK: Archive = Archive {
    file: "dxvk-3.1.1.tar.gz",
    url: "https://github.com/doitsujin/dxvk/releases/download/v3.1.1/dxvk-3.1.1.tar.gz",
    md5: "dfca4e1ee1399ebd365c1b1b919dc78a",
    sha256: "40565b4a724aadc4433fa4e010b4b23916d9b1f1baeee64e17186db94f54e608",
    size: 18_041_512,
};

pub const DXVK_DLLS: &[&str] = &["d3d8", "d3d9", "d3d10core", "d3d11", "dxgi"];

/// Unpack DXVK's `x64` DLLs into system32 and `x32` into syswow64.
pub fn install_dxvk(archive: &Path, wine: &Wine) -> Result<Vec<String>, StepError> {
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut tar = tar::Archive::new(gz);
    let mut installed = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let parts: Vec<String> = path.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        // dxvk-<ver>/<x64|x32>/<dll>
        let [_, arch, file] = parts.as_slice() else { continue };
        let Some(stem) = file.strip_suffix(".dll") else { continue };
        if !DXVK_DLLS.contains(&stem) {
            continue;
        }
        let dir = match arch.as_str() {
            "x64" => wine.system32(),
            "x32" => wine.syswow64(),
            _ => continue,
        };
        std::fs::create_dir_all(&dir)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let target = dir.join(file);
        let _ = std::fs::remove_file(&target);
        std::fs::write(&target, bytes)?;
        installed.push(format!("{arch}/{file}"));
    }
    if installed.len() != DXVK_DLLS.len() * 2 {
        return Err(StepError::msg(format!("the DXVK archive is incomplete (found {})", installed.join(", "))));
    }
    Ok(installed)
}

pub fn dxvk_overrides() -> Vec<(String, String, RegData)> {
    DXVK_DLLS
        .iter()
        .map(|d| (r"HKEY_CURRENT_USER\Software\Wine\DllOverrides".to_string(), (*d).to_string(), RegData::Sz("native".into())))
        .collect()
}

pub const DIRECT3D_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Direct3D";
pub const DIRECT2D_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Direct2D";
pub const DRIVERS_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Drivers";

/// `renderer=gl`, `MaxVersionGL=0x30002`, and the Wayland driver the way Bottles' `wayland: true`
/// sets it (`Graphics=x11,wayland`). The Direct2D value is deleted separately.
pub fn settings() -> Vec<(String, String, RegData)> {
    vec![
        (DIRECT3D_KEY.into(), "renderer".into(), RegData::Sz("gl".into())),
        (DIRECT3D_KEY.into(), "MaxVersionGL".into(), RegData::Dword(0x30002)),
        (DRIVERS_KEY.into(), "Graphics".into(), RegData::Sz("x11,wayland".into())),
    ]
}
