//! Runtime self-check: the runner starts, and a 32-bit Windows program runs in a scratch
//! prefix (system-dependencies: Runtime self-check).

use std::path::Path;

use crate::core::Cancel;
use crate::core::proc::{OpLog, ProcError, Stream, run};
use crate::prefix::wine::Wine;

#[derive(Debug, thiserror::Error)]
pub enum SelfCheckError {
    #[error("the Wine runner does not start: {detail}")]
    Version { detail: String },
    #[error("Wine could not create a test prefix: {detail}")]
    Boot { detail: String },
    #[error("Wine cannot run 32-bit Windows programs. Usually 32-bit system libraries are missing.\n{detail}")]
    Run32 { detail: String },
    #[error("cancelled")]
    Cancelled,
}

fn proc_err(e: ProcError, make: impl FnOnce(String) -> SelfCheckError) -> SelfCheckError {
    match e {
        ProcError::Cancelled => SelfCheckError::Cancelled,
        other => make(other.to_string()),
    }
}

/// Run the check. Returns the runner's version string. `extra_env` is applied to every Wine
/// process (tests use it to break the 32-bit loader).
pub fn run_selfcheck(
    runner: &Path,
    scratch_prefix: &Path,
    extra_env: &[(&str, &str)],
    log: Option<&OpLog>,
    on_line: &mut dyn FnMut(Stream, &str),
    cancel: &Cancel,
) -> Result<String, SelfCheckError> {
    let wine = Wine::new(runner, scratch_prefix);
    let with_env = |mut c: crate::core::proc::Cmd| {
        for (k, v) in extra_env {
            c = c.env(*k, *v);
        }
        c
    };

    let out = run(&with_env(wine.wine(["--version"])), log, on_line, cancel)
        .map_err(|e| proc_err(e, |detail| SelfCheckError::Version { detail }))?;
    let version = out.lines.iter().rev().find(|l| l.starts_with("wine")).cloned();
    let Some(version) = version.filter(|_| out.status.success()) else {
        return Err(SelfCheckError::Version { detail: out.tail(20) });
    };

    if let Some(parent) = scratch_prefix.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let boot = with_env(wine.wine(["wineboot", "-u"]).env("WINEDLLOVERRIDES", "winemenubuilder.exe=d;mscoree=d;mshtml=d"));
    let out = run(&boot, log, on_line, cancel).map_err(|e| proc_err(e, |detail| SelfCheckError::Boot { detail }))?;
    let _ = run(&with_env(wine.wineserver_wait()), log, on_line, cancel);
    if !out.status.success() || !wine.syswow64().join("reg.exe").exists() {
        let detail = out.tail(30);
        wine.kill_all();
        // A boot that dies on the 32-bit loader is a 32-bit problem, not a prefix problem.
        return Err(if detail.contains("ld-linux.so.2") || detail.contains("i386") {
            SelfCheckError::Run32 { detail }
        } else {
            SelfCheckError::Boot { detail }
        });
    }

    let reg32 = with_env(wine.wine([
        r"C:\windows\syswow64\reg.exe",
        "query",
        r"HKLM\Software\Microsoft\Windows NT\CurrentVersion",
        "/v",
        "CurrentVersion",
    ]));
    let out = run(&reg32, log, on_line, cancel);
    wine.kill_all();
    let out = out.map_err(|e| proc_err(e, |detail| SelfCheckError::Run32 { detail }))?;
    if !out.status.success() || !out.lines.iter().any(|l| l.contains("CurrentVersion") && l.contains("REG_SZ")) {
        return Err(SelfCheckError::Run32 { detail: out.tail(30) });
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Needs an extracted runner: `LOIS_TEST_RUNNER=/path/to/soda cargo test -- --ignored selfcheck`.
    fn test_runner() -> Option<PathBuf> {
        std::env::var_os("LOIS_TEST_RUNNER").map(PathBuf::from)
    }

    #[test]
    #[ignore]
    fn selfcheck_passes_with_real_runner() {
        let runner = test_runner().expect("set LOIS_TEST_RUNNER");
        let tmp = tempfile::tempdir().unwrap();
        let v = run_selfcheck(&runner, &tmp.path().join("pfx"), &[], None, &mut |_, _| {}, &Cancel::new()).unwrap();
        assert!(v.starts_with("wine-"), "{v}");
    }

    /// A truncated 32-bit `libc.so.6` first on `LD_LIBRARY_PATH` is skipped by the 64-bit
    /// loader (wrong ELF class) but breaks every 32-bit process, like missing 32-bit glibc.
    #[test]
    #[ignore]
    fn selfcheck_fails_when_32_bit_loading_breaks() {
        let runner = test_runner().expect("set LOIS_TEST_RUNNER");
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("badlib");
        std::fs::create_dir_all(&bad).unwrap();
        let real = std::fs::read("/usr/lib32/libc.so.6").expect("32-bit glibc in the test system");
        std::fs::write(bad.join("libc.so.6"), &real[..64]).unwrap();
        let err = run_selfcheck(
            &runner,
            &tmp.path().join("pfx"),
            &[("LD_LIBRARY_PATH", bad.to_str().unwrap())],
            None,
            &mut |_, _| {},
            &Cancel::new(),
        )
        .unwrap_err();
        match err {
            SelfCheckError::Run32 { detail } => {
                eprintln!("captured: {detail}");
                assert!(!detail.is_empty())
            }
            other => panic!("expected a 32-bit failure, got {other:?}"),
        }
    }
}
