//! The loader cache as listed by `ldconfig -p`, split into 64-bit and 32-bit entries
//! (design D7). One of the sources `loader::scan` searches.

use std::collections::HashSet;
use std::path::PathBuf;

use super::maps::Bits;
use crate::core::proc::{Cmd, capture, is_executable, which};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibSet {
    pub lib64: HashSet<String>,
    pub lib32: HashSet<String>,
}

impl LibSet {
    pub fn insert(&mut self, name: &str, bits: Bits) {
        match bits {
            Bits::B64 => self.lib64.insert(name.to_string()),
            Bits::B32 => self.lib32.insert(name.to_string()),
        };
    }

    pub fn is_empty(&self) -> bool {
        self.lib64.is_empty() && self.lib32.is_empty()
    }
}

/// One `ldconfig -p` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bits: Bits,
    pub path: PathBuf,
}

/// Parse `ldconfig -p`. Lines look like
/// `\tlibzstd.so.1 (libc6,x86-64) => /usr/lib/libzstd.so.1`; a bare `(libc6)` tag (optionally
/// followed by `, OS ABI: …`) marks a 32-bit x86 library. Other ABIs (x32, other arches) are ignored.
pub fn parse_entries(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((name, rest)) = line.split_once(" (") else { continue };
        let Some((tag, rest)) = rest.split_once(')') else { continue };
        let mut parts = tag.split(',').map(str::trim);
        if parts.next() != Some("libc6") {
            continue;
        }
        let bits = match parts.next() {
            Some("x86-64") => Bits::B64,
            None => Bits::B32,
            Some(p) if p.starts_with("OS ABI") => Bits::B32,
            _ => continue,
        };
        let path = rest.split_once("=>").map(|(_, p)| PathBuf::from(p.trim())).unwrap_or_default();
        out.push(Entry { name: name.to_string(), bits, path });
    }
    out
}

/// Every name in the listing, without looking at the files (fixture tests).
#[cfg(test)]
pub fn parse(text: &str) -> LibSet {
    let mut set = LibSet::default();
    for e in parse_entries(text) {
        set.insert(&e.name, e.bits);
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

/// The system's loader cache, or nothing when `ldconfig` is missing or fails (NixOS has no
/// cache; musl's `ldconfig` has no `-p`).
pub fn cache_entries() -> Vec<Entry> {
    let Some(ldconfig) = ldconfig_path() else {
        log::info!("ldconfig not found; searching the library directories only");
        return Vec::new();
    };
    match capture(&Cmd::new(ldconfig).arg("-p").env("LC_ALL", "C")) {
        Ok((_, out)) => parse_entries(&out),
        Err(e) => {
            log::info!("ldconfig -p failed ({e}); searching the library directories only");
            Vec::new()
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

    #[test]
    fn entries_keep_paths() {
        let e = parse_entries("\tlibfoo.so.1 (libc6,x86-64) => /usr/lib/libfoo.so.1\n");
        assert_eq!(e, [Entry { name: "libfoo.so.1".into(), bits: Bits::B64, path: "/usr/lib/libfoo.so.1".into() }]);
    }
}
