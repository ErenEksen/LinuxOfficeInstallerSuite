//! Installed shared libraries from `ldconfig -p`, split into 64-bit and 32-bit sets (design D7).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::core::proc::{Cmd, capture, is_executable, which};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibSet {
    pub lib64: HashSet<String>,
    pub lib32: HashSet<String>,
}

/// Parse `ldconfig -p`. Lines look like
/// `\tlibzstd.so.1 (libc6,x86-64) => /usr/lib/libzstd.so.1`; a bare `(libc6)` tag (optionally
/// followed by `, OS ABI: …`) marks a 32-bit x86 library. Other ABIs (x32, other arches) are ignored.
pub fn parse(text: &str) -> LibSet {
    let mut set = LibSet::default();
    for line in text.lines() {
        let line = line.trim();
        let Some((name, rest)) = line.split_once(" (") else { continue };
        let Some((tag, _)) = rest.split_once(')') else { continue };
        let mut parts = tag.split(',').map(str::trim);
        if parts.next() != Some("libc6") {
            continue;
        }
        match parts.next() {
            Some("x86-64") => {
                set.lib64.insert(name.to_string());
            }
            None => {
                set.lib32.insert(name.to_string());
            }
            Some(p) if p.starts_with("OS ABI") => {
                set.lib32.insert(name.to_string());
            }
            _ => {}
        }
    }
    set
}

/// Locate `ldconfig`; on Debian it lives in `/sbin`, outside a normal user's `PATH`.
fn ldconfig_path() -> Option<PathBuf> {
    which("ldconfig").or_else(|| {
        ["/sbin/ldconfig", "/usr/sbin/ldconfig", "/usr/bin/ldconfig"]
            .iter()
            .map(PathBuf::from)
            .find(|p| is_executable(p))
    })
}

/// Scan the system's library cache.
pub fn scan() -> LibSet {
    let Some(ldconfig) = ldconfig_path() else {
        log::warn!("ldconfig not found; every library will be reported missing");
        return LibSet::default();
    };
    match capture(&Cmd::new(ldconfig).arg("-p").env("LC_ALL", "C")) {
        Ok((_, out)) => {
            let mut set = parse(&out);
            // Wine's 32-bit loader is the ELF interpreter path itself; trust the file too.
            if Path::new("/lib/ld-linux.so.2").exists() {
                set.lib32.insert("ld-linux.so.2".into());
            }
            set
        }
        Err(e) => {
            log::warn!("ldconfig -p failed: {e}");
            LibSet::default()
        }
    }
}

/// Whether a helper tool is on `PATH`.
pub fn has_tool(name: &str) -> bool {
    which(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AFTER: &str = include_str!("../../tests/fixtures/ldconfig-after-lib32.txt");
    const BEFORE: &str = include_str!("../../tests/fixtures/ldconfig-before-lib32.txt");

    #[test]
    fn splits_64_and_32_bit() {
        let s = parse(AFTER);
        assert!(s.lib64.contains("libunwind.so.8"));
        assert!(!s.lib32.contains("libunwind.so.8"));
        assert!(s.lib32.contains("ld-linux.so.2"));
        assert!(s.lib64.contains("ld-linux-x86-64.so.2"));
        assert!(s.lib32.contains("libgcrypt.so.20") && s.lib64.contains("libgcrypt.so.20"));
        assert!(s.lib64.len() > 500 && s.lib32.len() > 200);
    }

    #[test]
    fn before_lib32_has_no_32_bit_libraries() {
        let s = parse(BEFORE);
        assert!(s.lib32.is_empty());
        assert!(!s.lib32.contains("ld-linux.so.2"));
        assert!(s.lib64.contains("libfreetype.so.6"));
    }

    #[test]
    fn handles_os_abi_and_foreign_tags() {
        let s = parse(
            "3 libs found\n\tlibfoo.so.1 (libc6, OS ABI: Linux 3.2.0) => /lib/i386-linux-gnu/libfoo.so.1\n\
             \tlibbar.so.1 (libc6,x32) => /libx32/libbar.so.1\n\
             \tlibbaz.so.2 (libc6,x86-64, OS ABI: Linux 3.2.0) => /lib/x86_64-linux-gnu/libbaz.so.2\n",
        );
        assert!(s.lib32.contains("libfoo.so.1"));
        assert!(!s.lib32.contains("libbar.so.1") && !s.lib64.contains("libbar.so.1"));
        assert!(s.lib64.contains("libbaz.so.2"));
    }
}
