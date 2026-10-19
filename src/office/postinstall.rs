//! Fixes applied after Office setup succeeds, from the Bottles recipe
//! (office-installation: Post-install fixes). Nothing else about licensing is touched.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::detect::office16_dir;
use crate::core::Cancel;
use crate::core::proc::OpLog;
use crate::prefix::steps::StepError;
use crate::prefix::wine::{RegData, Wine};

const C2R_DIR: &str = "drive_c/Program Files/Common Files/Microsoft Shared/ClickToRun";
pub const LOCKER: &str = "drive_c/ProgramData/Microsoft/Office/ClickToRunPackageLocker";

/// Copy `AppvIsvSubsystems64.dll` and `C2R64.dll` from the ClickToRun folder into `Office16`.
pub fn copy_c2r_dlls(prefix: &Path) -> Result<Vec<String>, StepError> {
    let office16 = office16_dir(prefix).ok_or_else(|| StepError::msg("Office16 folder not found after setup"))?;
    let src = prefix.join(C2R_DIR);
    let mut copied = Vec::new();
    for dll in ["AppvIsvSubsystems64.dll", "C2R64.dll"] {
        let from = src.join(dll);
        if !from.exists() {
            return Err(StepError::msg(format!("{dll} is missing from the ClickToRun folder")));
        }
        let to = office16.join(dll);
        std::fs::copy(&from, &to)?;
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o644))?;
        copied.push(dll.to_string());
    }
    Ok(copied)
}

/// `sppc` → `builtin,native`, and `LicensingNext\<ProductID>` = 2.
pub fn registry_fixes(product_id: &str) -> Vec<(String, String, RegData)> {
    vec![
        (r"HKEY_CURRENT_USER\Software\Wine\DllOverrides".into(), "sppc".into(), RegData::Sz("builtin,native".into())),
        (
            r"HKEY_CURRENT_USER\Software\Microsoft\Office\16.0\Common\Licensing\LicensingNext".into(),
            product_id.into(),
            RegData::Dword(2),
        ),
    ]
}

/// Reset the package locker to 0770 (the launch wrapper does this too, before each start).
pub fn fix_locker(prefix: &Path) -> std::io::Result<()> {
    let locker = prefix.join(LOCKER);
    if locker.exists() {
        std::fs::set_permissions(&locker, std::fs::Permissions::from_mode(0o770))?;
    }
    Ok(())
}

pub fn apply(wine: &Wine, product_id: &str, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    let copied = copy_c2r_dlls(&wine.prefix)?;
    log.line(&format!("copied {} into Office16", copied.join(", ")));
    wine.reg_import(&registry_fixes(product_id), Some(log), cancel)?;
    fix_locker(&wine.prefix)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_dlls_on_a_fake_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        let c2r = p.join(C2R_DIR);
        let o16 = p.join("drive_c/Program Files/Microsoft Office/root/Office16");
        std::fs::create_dir_all(&c2r).unwrap();
        std::fs::create_dir_all(&o16).unwrap();
        std::fs::write(o16.join("WINWORD.EXE"), "MZ").unwrap();
        assert!(copy_c2r_dlls(p).is_err(), "missing source DLLs are an error");
        std::fs::write(c2r.join("AppvIsvSubsystems64.dll"), "a").unwrap();
        std::fs::write(c2r.join("C2R64.dll"), "b").unwrap();
        assert_eq!(copy_c2r_dlls(p).unwrap(), ["AppvIsvSubsystems64.dll", "C2R64.dll"]);
        assert_eq!(std::fs::read_to_string(o16.join("C2R64.dll")).unwrap(), "b");
        let locker = p.join(LOCKER);
        std::fs::create_dir_all(&locker).unwrap();
        std::fs::set_permissions(&locker, std::fs::Permissions::from_mode(0o500)).unwrap();
        fix_locker(p).unwrap();
        assert_eq!(locker.metadata().unwrap().permissions().mode() & 0o777, 0o770);
    }

    #[test]
    fn registry_values() {
        let r = registry_fixes("O365HomePremRetail");
        assert_eq!(r[0].2, RegData::Sz("builtin,native".into()));
        assert!(r[1].0.ends_with(r"Licensing\LicensingNext"));
        assert_eq!((r[1].1.as_str(), &r[1].2), ("O365HomePremRetail", &RegData::Dword(2)));
    }
}
