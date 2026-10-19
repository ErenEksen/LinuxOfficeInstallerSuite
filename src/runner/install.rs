//! Download, verify and extract a runner (wine-runner-management: Verified runner download).
//!
//! The archive is checked against the release's `SHA256SUMS` before extraction, extracted
//! into a hidden temp directory under `runners/`, then renamed into `runners/<tag>` in one
//! step. Any failure removes the temp directory, so no partial runner is ever visible.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use reqwest::blocking::Client;

use super::github::Release;
use crate::core::Cancel;
use crate::core::download::{Checksum, DownloadError, download, get_text};
use crate::core::paths::Paths;
use crate::core::proc::OpLog;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Download,
    Extract,
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("{0} has no SHA256SUMS file, so it cannot be verified and is not installed")]
    NoChecksums(String),
    #[error("SHA256SUMS of {tag} has no entry for {file}")]
    NotInSums { tag: String, file: String },
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("could not extract the runner: {0}")]
    Extract(io::Error),
    #[error("the archive does not contain a Wine runner (bin/wine is missing)")]
    NotARunner,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Find the hash for `file` in a `sha256sum`-style list (`<hex>  <name>` or `<hex> *<name>`).
pub fn parse_sums(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        let name = name.rsplit('/').next().unwrap_or(name);
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

/// Install `release` into `runners/<tag>`. Returns the runner directory. An already
/// installed runner is left as is.
pub fn install_runner(
    client: &Client,
    paths: &Paths,
    release: &Release,
    progress: &mut dyn FnMut(Phase, u64, Option<u64>),
    log: Option<&OpLog>,
    cancel: &Cancel,
) -> Result<PathBuf, RunnerError> {
    let dest = paths.runner(&release.tag);
    if dest.join("bin/wine").exists() {
        return Ok(dest);
    }
    let note = |s: &str| {
        if let Some(l) = log {
            l.line(s)
        }
    };

    let sums_url = release.sums_url.as_deref().ok_or_else(|| RunnerError::NoChecksums(release.tag.clone()))?;
    note(&format!("fetching {sums_url}"));
    let sums = get_text(client, sums_url)?;
    let sha = parse_sums(&sums, &release.archive_name)
        .ok_or_else(|| RunnerError::NotInSums { tag: release.tag.clone(), file: release.archive_name.clone() })?;
    note(&format!("expected sha256 {sha}"));

    let archive = paths.cache().join("runners").join(&release.archive_name);
    note(&format!("downloading {} to {}", release.archive_url, archive.display()));
    download(
        client,
        &release.archive_url,
        &archive,
        &[Checksum::Sha256(sha)],
        &mut |d, t| progress(Phase::Download, d, t),
        cancel,
    )
    .map_err(|e| match e {
        DownloadError::Cancelled => RunnerError::Cancelled,
        other => other.into(),
    })?;
    note("checksum verified");

    std::fs::create_dir_all(paths.runners())?;
    let tmp = paths.runners().join(format!(".installing-{}-{}", release.tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let result = extract(&archive, &tmp, progress, cancel).and_then(|root| {
        if !root.join("bin/wine").exists() {
            return Err(RunnerError::NotARunner);
        }
        std::fs::rename(&root, &dest)?;
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result?;
    note(&format!("installed {}", dest.display()));
    // The extracted runner is the useful copy; drop the 200+ MB archive.
    let _ = std::fs::remove_file(&archive);
    Ok(dest)
}

/// Counts compressed bytes read, for extraction progress.
struct Counting<R> {
    inner: R,
    read: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.set(self.read.get() + n as u64);
        Ok(n)
    }
}

/// Extract a `.tar.xz` into `tmp` and return its single top-level directory (or `tmp`
/// itself when the archive has no wrapping directory).
fn extract(
    archive: &Path,
    tmp: &Path,
    progress: &mut dyn FnMut(Phase, u64, Option<u64>),
    cancel: &Cancel,
) -> Result<PathBuf, RunnerError> {
    std::fs::create_dir_all(tmp)?;
    let total = archive.metadata()?.len();
    let file = File::open(archive)?;
    let read = Rc::new(Cell::new(0));
    let xz = xz2::read::XzDecoder::new(Counting { inner: file, read: read.clone() });
    let mut tar = tar::Archive::new(xz);
    tar.set_preserve_permissions(true);
    tar.set_preserve_mtime(true);
    tar.set_overwrite(true);
    for entry in tar.entries().map_err(RunnerError::Extract)? {
        if cancel.is_cancelled() {
            return Err(RunnerError::Cancelled);
        }
        let mut entry = entry.map_err(RunnerError::Extract)?;
        // unpack_in refuses paths that escape `tmp`.
        entry.unpack_in(tmp).map_err(RunnerError::Extract)?;
        progress(Phase::Extract, read.get(), Some(total));
    }
    let mut tops = std::fs::read_dir(tmp)?.filter_map(|e| e.ok()).collect::<Vec<_>>();
    if tops.len() == 1 && tops[0].file_type()?.is_dir() {
        Ok(tops.remove(0).path())
    } else {
        Ok(tmp.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::download::http_client;
    use crate::core::download::testserver::{Route, Server};
    use sha2::{Digest, Sha256};

    fn fake_runner_archive(top: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_bytes);
            let script = b"#!/bin/sh\necho wine-11.0\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(script.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, format!("{top}/bin/wine"), &script[..]).unwrap();
            let lib = vec![7u8; 50_000];
            let mut h = tar::Header::new_gnu();
            h.set_size(lib.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, format!("{top}/lib/wine/x86_64-unix/ntdll.so"), &lib[..]).unwrap();
            b.finish().unwrap();
        }
        let mut out = Vec::new();
        let mut enc = xz2::read::XzEncoder::new(&tar_bytes[..], 1);
        enc.read_to_end(&mut out).unwrap();
        out
    }

    fn setup(sums_for: impl Fn(&str) -> String) -> (Server, Release, Vec<u8>) {
        let srv = Server::start();
        let tag = "soda-test-1";
        let archive = fake_runner_archive(&format!("{tag}-x86_64"));
        let name = format!("{tag}-x86_64.tar.xz");
        let sha = hex::encode(Sha256::digest(&archive));
        let archive_url = srv.route(&format!("/{name}"), Route::ok(archive.clone()));
        let sums_url = srv.route("/SHA256SUMS", Route::ok(format!("{}  {name}\n00  other\n", sums_for(&sha))));
        let release = Release {
            tag: tag.into(),
            prerelease: true,
            date: String::new(),
            published_at: String::new(),
            archive_url,
            archive_name: name,
            archive_size: archive.len() as u64,
            sums_url: Some(sums_url),
        };
        (srv, release, archive)
    }

    #[test]
    fn installs_and_reports_progress() {
        let (_srv, release, _) = setup(|sha| sha.to_string());
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let mut phases = Vec::new();
        let dir = install_runner(&http_client(), &paths, &release, &mut |p, _, _| phases.push(p), None, &Cancel::new())
            .unwrap();
        assert_eq!(dir, paths.runner("soda-test-1"));
        assert!(crate::core::proc::is_executable(&dir.join("bin/wine")));
        assert!(dir.join("lib/wine/x86_64-unix/ntdll.so").exists());
        assert!(phases.contains(&Phase::Download) && phases.contains(&Phase::Extract));
        let leftovers: Vec<_> = std::fs::read_dir(paths.runners()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("soda-test-1")]);
    }

    #[test]
    fn tampered_sums_leave_no_directory() {
        let (_srv, release, _) = setup(|_| "ab".repeat(32));
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let err = install_runner(&http_client(), &paths, &release, &mut |_, _, _| {}, None, &Cancel::new()).unwrap_err();
        assert!(matches!(err, RunnerError::Download(DownloadError::Mismatch { .. })), "{err:?}");
        assert!(!paths.runner("soda-test-1").exists());
        let runners_empty = std::fs::read_dir(paths.runners()).map(|mut d| d.next().is_none()).unwrap_or(true);
        assert!(runners_empty);
        assert!(!paths.cache().join("runners").join(&release.archive_name).exists(), "bad archive deleted");
    }

    #[test]
    fn missing_sums_entry_and_no_sums() {
        let (_srv, mut release, _) = setup(|sha| sha.to_string());
        release.archive_name = "renamed.tar.xz".into();
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let err = install_runner(&http_client(), &paths, &release, &mut |_, _, _| {}, None, &Cancel::new()).unwrap_err();
        assert!(matches!(err, RunnerError::NotInSums { .. }));
        release.sums_url = None;
        let err = install_runner(&http_client(), &paths, &release, &mut |_, _, _| {}, None, &Cancel::new()).unwrap_err();
        assert!(matches!(err, RunnerError::NoChecksums(_)));
    }

    #[test]
    fn parses_sha256sums() {
        let text = "aaaa  short\n0123456789abcdef0123456789abcdef0123456789ABCDEF0123456789abcdef *soda-x86_64.tar.xz\n";
        assert_eq!(
            parse_sums(text, "soda-x86_64.tar.xz").unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert_eq!(parse_sums(text, "short"), None);
    }
}
