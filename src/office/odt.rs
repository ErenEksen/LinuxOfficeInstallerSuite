//! The Office Deployment Tool, downloaded only from Microsoft's CDN at install time
//! (office-installation: Official ODT only).

use std::io::Read;
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;

use crate::core::Cancel;
use crate::core::download::{DownloadError, download};
use crate::core::paths::Paths;
use crate::core::proc::OpLog;
use crate::prefix::wine::Wine;

pub const ODT_URL: &str = "https://officecdn.microsoft.com/pr/wsus/setup.exe";

/// True when the file starts with an `MZ` header whose `e_lfanew` points at a `PE\0\0` signature.
pub fn is_pe(path: &Path) -> bool {
    let mut buf = Vec::new();
    let Ok(f) = std::fs::File::open(path) else { return false };
    if f.take(4096).read_to_end(&mut buf).is_err() || buf.len() < 0x40 || &buf[..2] != b"MZ" {
        return false;
    }
    let off = u32::from_le_bytes([buf[0x3C], buf[0x3D], buf[0x3E], buf[0x3F]]) as usize;
    buf.get(off..off + 4) == Some(b"PE\0\0")
}

#[derive(Debug, thiserror::Error)]
pub enum OdtError {
    #[error("the Office Deployment Tool download failed: {0}")]
    Download(DownloadError),
    #[error("the downloaded Office Deployment Tool is not a Windows program; it was deleted")]
    NotPe,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Download the ODT into the cache (reusing a valid cached copy) and validate it.
pub fn fetch_odt(
    client: &Client,
    url: &str,
    cache_file: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
    log: Option<&OpLog>,
    cancel: &Cancel,
) -> Result<PathBuf, OdtError> {
    if cache_file.exists() && is_pe(cache_file) {
        if let Some(l) = log {
            l.line(&format!("using cached {}", cache_file.display()));
        }
        return Ok(cache_file.to_path_buf());
    }
    let _ = std::fs::remove_file(cache_file);
    if let Some(l) = log {
        l.line(&format!("downloading {url}"));
    }
    download(client, url, cache_file, &[], progress, cancel).map_err(|e| match e {
        DownloadError::Cancelled => OdtError::Cancelled,
        other => OdtError::Download(other),
    })?;
    if !is_pe(cache_file) {
        let _ = std::fs::remove_file(cache_file);
        return Err(OdtError::NotPe);
    }
    Ok(cache_file.to_path_buf())
}

pub fn cache_file(paths: &Paths) -> PathBuf {
    paths.cache().join("odt/setup.exe")
}

/// Copy the ODT into the prefix (`C:\windows\temp\office-deployment-tool\setup.exe`) and
/// return its Windows path.
pub fn stage_in_prefix(odt: &Path, wine: &Wine) -> std::io::Result<String> {
    let dir = wine.drive_c().join("windows/temp/office-deployment-tool");
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(odt, dir.join("setup.exe"))?;
    Ok(r"C:\windows\temp\office-deployment-tool\setup.exe".into())
}

pub const C2R_EXE: &str = r"C:\Program Files\Common Files\Microsoft Shared\ClickToRun\OfficeClickToRun.exe";

/// `sc create ClickToRunSvc …` as in the Bottles recipe.
pub fn service_create_cmd(wine: &Wine) -> crate::core::proc::Cmd {
    wine.wine([
        "sc".to_string(),
        "create".into(),
        "ClickToRunSvc".into(),
        "binPath=".into(),
        format!("\"{C2R_EXE}\" /service"),
        "DisplayName=".into(),
        "Microsoft Office Click-to-Run Service".into(),
        "start=".into(),
        "auto".into(),
        "type=".into(),
        "own".into(),
        "error=".into(),
        "normal".into(),
    ])
}

/// Exit codes that mean the service exists: 0, or 1073 (ERROR_SERVICE_EXISTS) which Wine's
/// `sc` reports truncated to a byte as 49.
pub fn service_ok(code: Option<i32>) -> bool {
    matches!(code, Some(0) | Some(49) | Some(1073))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::download::http_client;
    use crate::core::download::testserver::{Route, Server};

    fn fake_pe() -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[..2].copy_from_slice(b"MZ");
        b[0x3C] = 0x80;
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b
    }

    #[test]
    fn pe_check() {
        let tmp = tempfile::tempdir().unwrap();
        let pe = tmp.path().join("a.exe");
        std::fs::write(&pe, fake_pe()).unwrap();
        assert!(is_pe(&pe));
        let html = format!("{}/tests/fixtures/not-pe.html", env!("CARGO_MANIFEST_DIR"));
        assert!(!is_pe(Path::new(&html)));
        let mz_only = tmp.path().join("b.exe");
        let mut b = fake_pe();
        b[0x80] = b'X';
        std::fs::write(&mz_only, b).unwrap();
        assert!(!is_pe(&mz_only));
    }

    #[test]
    fn html_download_is_rejected_and_deleted() {
        let srv = Server::start();
        let html = std::fs::read(format!("{}/tests/fixtures/not-pe.html", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let url = srv.route("/setup.exe", Route::ok(html));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("odt/setup.exe");
        let err = fetch_odt(&http_client(), &url, &dest, &mut |_, _| {}, None, &Cancel::new()).unwrap_err();
        assert!(matches!(err, OdtError::NotPe));
        assert!(!dest.exists());

        let url = srv.route("/good.exe", Route::ok(fake_pe()));
        fetch_odt(&http_client(), &url, &dest, &mut |_, _| {}, None, &Cancel::new()).unwrap();
        assert!(is_pe(&dest));
    }

    #[test]
    fn service_codes() {
        assert!(service_ok(Some(0)) && service_ok(Some(49)) && service_ok(Some(1073)));
        assert!(!service_ok(Some(1)) && !service_ok(None));
        let w = Wine::new("/r", "/p");
        let d = service_create_cmd(&w).display();
        assert!(d.contains(r#"binPath= '"C:\Program Files\Common Files\Microsoft Shared\ClickToRun\OfficeClickToRun.exe" /service'"#), "{d}");
    }

    /// `cargo test -- --ignored live_odt` downloads the real ODT.
    #[test]
    #[ignore]
    fn live_odt_download() {
        let paths = Paths::from_env();
        let f = fetch_odt(&http_client(), ODT_URL, &cache_file(&paths), &mut |_, _| {}, None, &Cancel::new()).unwrap();
        assert!(is_pe(&f));
        eprintln!("{} bytes", f.metadata().unwrap().len());
    }

    /// Registering twice succeeds both times: `cargo test -- --ignored live_service`.
    #[test]
    #[ignore]
    fn live_service_twice() {
        let paths = Paths::from_env();
        let w = Wine::new(paths.active_runner_link(), paths.prefix());
        for round in 1..=2 {
            let out = crate::core::proc::run(&service_create_cmd(&w), None, &mut |_, _| {}, &Cancel::new()).unwrap();
            eprintln!("round {round}: {:?} {:?}", out.status.code(), out.lines.iter().filter(|l| !l.contains(":err:")).collect::<Vec<_>>());
            assert!(service_ok(out.status.code()), "round {round}");
        }
        let image = w.query_value(r"HKLM\System\CurrentControlSet\Services\ClickToRunSvc", "ImagePath").unwrap().unwrap();
        eprintln!("ImagePath: {image}");
        assert_eq!(image, format!("\"{C2R_EXE}\" /service"));
        w.kill_all();
    }
}
