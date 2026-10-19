//! Web components for Office's sign-in window.
//!
//! Office's Sign in window (account email entry) is an Internet Explorer web control,
//! which Wine provides as `mshtml` drawn with Wine Gecko. The prefix is created with
//! `mshtml` disabled (no Gecko prompt during `wineboot`), so Gecko is installed here and
//! `mshtml` registered afterwards; without both the window opens blank and closes.
//! The Edge WebView2 Runtime is installed too, for Office features that embed it
//! (add-ins); Chromium's sandbox does not work under Wine, so the launch wrapper sets
//! `--no-sandbox` for it.

use std::path::Path;

use super::redist::Archive;
use super::steps::StepError;
use super::wine::Wine;
use crate::core::Cancel;
use crate::core::proc::{OpLog, run_checked};

/// The Gecko version this Wine's `mshtml` asks for; the SHA-256s match the ones pinned in
/// the runner's `appwiz.cpl`.
pub const GECKO_X64: Archive = Archive {
    file: "wine-gecko-2.47.4-x86_64.msi",
    url: "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86_64.msi",
    md5: "8cc7018079a8db70e822320b69185515",
    sha256: "e590b7d988a32d6aa4cf1d8aa3aa3d33766fdd4cf4c89c2dcc2095ecb28d066f",
    size: 53_898_752,
};

pub const GECKO_X86: Archive = Archive {
    file: "wine-gecko-2.47.4-x86.msi",
    url: "https://dl.winehq.org/wine/wine-gecko/2.47.4/wine-gecko-2.47.4-x86.msi",
    md5: "f61c2a0065301ce329aa390342e70f14",
    sha256: "26cecc47706b091908f7f814bddb074c61beb8063318e9efc5a7f789857793d6",
    size: 55_187_968,
};

/// The standalone WebView2 Runtime installer (155.0.4283.62) behind Microsoft's
/// `go.microsoft.com/fwlink/?linkid=2124701`, pinned to the file it resolved to.
pub const WEBVIEW2: Archive = Archive {
    file: "MicrosoftEdgeWebView2RuntimeInstallerX64-155.0.4283.62.exe",
    url: "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/35871e19-d11b-432d-9d1e-7be0ff286540/MicrosoftEdgeWebView2RuntimeInstallerX64.exe",
    md5: "82725342d3a7fb62976796589eafd330",
    sha256: "0625fb5c1e75be58a164e14f95aa58a3e5badb859b3d17c952385f3e4a9ff3e8",
    size: 214_156_024,
};

/// Install one Gecko MSI silently.
pub fn install_gecko(wine: &Wine, msi: &Path, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    run_checked(&wine.wine(["msiexec", "/i"]).arg(msi).arg("/qn"), Some(log), &mut |_, _| {}, cancel)?;
    Ok(())
}

pub fn gecko_installed(wine: &Wine) -> bool {
    [wine.system32(), wine.syswow64()].iter().all(|d| d.join("gecko/2.47.4/wine_gecko").is_dir())
}

/// Register `mshtml` for both architectures. This writes the `text/html` content type
/// that tells Wine's web control to render HTML with it.
pub fn register_mshtml(wine: &Wine, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    for dll in [r"C:\windows\system32\mshtml.dll", r"C:\windows\syswow64\mshtml.dll"] {
        run_checked(&wine.wine(["regsvr32", "/s", dll]), Some(log), &mut |_, _| {}, cancel)?;
    }
    Ok(())
}

/// Install the WebView2 Runtime silently.
pub fn install_webview2(wine: &Wine, installer: &Path, log: &OpLog, cancel: &Cancel) -> Result<(), StepError> {
    run_checked(&wine.wine([installer.as_os_str()]).args(["/silent", "/install"]), Some(log), &mut |_, _| {}, cancel)?;
    if !webview2_installed(wine) {
        return Err(StepError::msg("the WebView2 Runtime installer finished, but the runtime is missing"));
    }
    Ok(())
}

pub fn webview2_installed(wine: &Wine) -> bool {
    std::fs::read_dir(wine.drive_c().join("Program Files (x86)/Microsoft/EdgeWebView/Application"))
        .map(|d| d.flatten().any(|e| e.path().join("msedgewebview2.exe").is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_pinned() {
        for a in [&GECKO_X64, &GECKO_X86, &WEBVIEW2] {
            assert_eq!(a.md5.len(), 32, "{}", a.file);
            assert_eq!(a.sha256.len(), 64, "{}", a.file);
            assert!(a.url.starts_with("https://") && a.size > 0);
        }
    }

    #[test]
    fn detects_installed_components() {
        let tmp = tempfile::tempdir().unwrap();
        let wine = Wine::new("/nonexistent", tmp.path());
        assert!(!gecko_installed(&wine) && !webview2_installed(&wine));
        for d in [wine.system32(), wine.syswow64()] {
            std::fs::create_dir_all(d.join("gecko/2.47.4/wine_gecko")).unwrap();
        }
        let app = wine.drive_c().join("Program Files (x86)/Microsoft/EdgeWebView/Application/155.0.4283.62");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("msedgewebview2.exe"), "").unwrap();
        assert!(gecko_installed(&wine) && webview2_installed(&wine));
    }
}
