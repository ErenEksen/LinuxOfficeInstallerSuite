//! Library lookup along the dynamic loader's search path (design D7): `LD_LIBRARY_PATH`
//! (and `NIX_LD_LIBRARY_PATH`, which nix-ld hands to the program), the `ld.so.cache`, the
//! `ld.so.conf` directories and the default directories. Only the requested sonames are
//! probed, and every file found is checked for the right ELF class and machine, so a stale
//! cache entry or a library of the other width never counts.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::ldconfig::{self, Entry, LibSet};
use super::maps::Bits;

/// Directories glibc searches on x86-64 distros (its trusted directories plus the multilib
/// and multiarch layouts of Arch, Fedora and Debian).
pub const DEFAULT_DIRS: &[&str] = &[
    "/lib",
    "/usr/lib",
    "/lib64",
    "/usr/lib64",
    "/lib32",
    "/usr/lib32",
    "/lib/x86_64-linux-gnu",
    "/usr/lib/x86_64-linux-gnu",
    "/lib/i386-linux-gnu",
    "/usr/lib/i386-linux-gnu",
];

/// Everything the lookup reads from the system, gathered once so `resolve` stays pure.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub env_dirs: Vec<PathBuf>,
    pub cache: Vec<Entry>,
    pub conf_dirs: Vec<PathBuf>,
    pub default_dirs: Vec<PathBuf>,
}

impl Sources {
    pub fn system() -> Sources {
        let mut env_dirs = Vec::new();
        for var in ["LD_LIBRARY_PATH", "NIX_LD_LIBRARY_PATH"] {
            if let Some(v) = std::env::var_os(var) {
                env_dirs.extend(split_search_path(&v.to_string_lossy()));
            }
        }
        Sources {
            env_dirs,
            cache: ldconfig::cache_entries(),
            conf_dirs: ld_so_conf_dirs(Path::new("/etc/ld.so.conf")),
            default_dirs: DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        }
    }
}

/// x86 width of an ELF shared object, or `None` for anything else (missing file, linker
/// script, other architecture).
pub fn elf_bits(path: &Path) -> Option<Bits> {
    let mut head = [0u8; 20];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    // Magic, then EI_CLASS (1 = 32-bit, 2 = 64-bit), EI_DATA (1 = little-endian), e_machine.
    if head[..4] != *b"\x7fELF" || head[5] != 1 {
        return None;
    }
    match (head[4], u16::from_le_bytes([head[18], head[19]])) {
        (1, 3) => Some(Bits::B32),  // EM_386
        (2, 62) => Some(Bits::B64), // EM_X86_64
        _ => None,
    }
}

/// `LD_LIBRARY_PATH` syntax: `:` or `;` separated; empty entries (the current directory) are skipped.
pub fn split_search_path(v: &str) -> Vec<PathBuf> {
    v.split([':', ';']).filter(|d| !d.is_empty()).map(PathBuf::from).collect()
}

/// Which of `wanted` the loader can find, per width.
pub fn resolve(wanted: &[&str], src: &Sources) -> LibSet {
    let mut set = LibSet::default();
    let dirs = src.env_dirs.iter().chain(&src.conf_dirs).chain(&src.default_dirs);
    for dir in dirs {
        for name in wanted {
            if let Some(bits) = elf_bits(&dir.join(name)) {
                set.insert(name, bits);
            }
        }
    }
    for e in src.cache.iter().filter(|e| wanted.contains(&e.name.as_str())) {
        if elf_bits(&e.path) == Some(e.bits) {
            set.insert(&e.name, e.bits);
        }
    }
    set
}

/// Search the running system.
pub fn scan(wanted: &[&str]) -> LibSet {
    resolve(wanted, &Sources::system())
}

/// Directories listed in an `ld.so.conf`, following `include` lines.
pub fn ld_so_conf_dirs(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    read_conf(path, 0, &mut out);
    out
}

fn read_conf(path: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    // glibc has no loop protection; a depth limit is enough against self-includes.
    if depth > 8 {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else { return };
    let base = path.parent().unwrap_or(Path::new("/"));
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some(pattern) = line.strip_prefix("include").filter(|r| r.starts_with(char::is_whitespace)) {
            for pat in pattern.split_whitespace() {
                for file in glob(&base.join(pat)) {
                    read_conf(&file, depth + 1, out);
                }
            }
        } else if line.starts_with("hwcap") {
            continue;
        } else {
            // Entries are separated by whitespace, `:` or `,`; a legacy `=TYPE` suffix is dropped.
            for dir in line.split([' ', '\t', ':', ',']).filter(|d| d.starts_with('/')) {
                let dir = PathBuf::from(dir.split('=').next().unwrap_or(dir));
                if !out.contains(&dir) {
                    out.push(dir);
                }
            }
        }
    }
}

/// Expand `*` and `?` in the last path component, sorted like glob(3).
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (pattern.parent(), pattern.file_name().and_then(|n| n.to_str())) else {
        return Vec::new();
    };
    if !name.contains(['*', '?']) {
        return if pattern.is_file() { vec![pattern.to_path_buf()] } else { Vec::new() };
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && wildcard(name, n)))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn wildcard(pattern: &str, name: &str) -> bool {
    match pattern.chars().next() {
        None => name.is_empty(),
        Some('*') => (0..=name.len()).filter(|i| name.is_char_boundary(*i)).any(|i| wildcard(&pattern[1..], &name[i..])),
        Some(c) => {
            let mut rest = name.chars();
            match rest.next() {
                Some(n) if c == '?' || c == n => wildcard(&pattern[c.len_utf8()..], rest.as_str()),
                _ => false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ELF header: class, little-endian, e_machine.
    fn elf(class: u8, machine: u16) -> Vec<u8> {
        let mut h = vec![0x7f, b'E', b'L', b'F', class, 1, 1];
        h.resize(18, 0);
        h.extend(machine.to_le_bytes());
        h.resize(64, 0);
        h
    }

    #[test]
    fn reads_elf_class_and_machine() {
        let tmp = tempfile::tempdir().unwrap();
        let p = |n: &str| tmp.path().join(n);
        std::fs::write(p("64"), elf(2, 62)).unwrap();
        std::fs::write(p("32"), elf(1, 3)).unwrap();
        std::fs::write(p("arm"), elf(2, 183)).unwrap();
        std::fs::write(p("script"), "/* GNU ld script */\nGROUP ( libc.so.6 )\n").unwrap();
        assert_eq!(elf_bits(&p("64")), Some(Bits::B64));
        assert_eq!(elf_bits(&p("32")), Some(Bits::B32));
        assert_eq!(elf_bits(&p("arm")), None);
        assert_eq!(elf_bits(&p("script")), None);
        assert_eq!(elf_bits(&p("missing")), None);
    }

    #[test]
    fn resolves_from_every_source_by_width() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |d: &str| {
            let d = tmp.path().join(d);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (env, conf, lib, lib32, stale) = (mk("env"), mk("conf"), mk("lib"), mk("lib32"), mk("stale"));
        std::fs::write(env.join("libA.so.1"), elf(1, 3)).unwrap();
        std::fs::write(conf.join("libB.so.1"), elf(2, 62)).unwrap();
        std::fs::write(lib.join("libC.so.1"), elf(2, 62)).unwrap();
        std::fs::write(lib32.join("libC.so.1"), elf(1, 3)).unwrap();
        // A 64-bit file in a directory the 32-bit loader also searches stays 64-bit only.
        std::fs::write(lib.join("libD.so.1"), elf(2, 62)).unwrap();
        std::fs::write(stale.join("libF.so.1"), elf(2, 62)).unwrap();
        let src = Sources {
            env_dirs: vec![env],
            cache: vec![
                Entry { name: "libE.so.1".into(), bits: Bits::B64, path: tmp.path().join("gone/libE.so.1") },
                // Cache says 32-bit, file is 64-bit.
                Entry { name: "libF.so.1".into(), bits: Bits::B32, path: stale.join("libF.so.1") },
            ],
            conf_dirs: vec![conf],
            default_dirs: vec![lib, lib32],
        };
        let s = resolve(&["libA.so.1", "libB.so.1", "libC.so.1", "libD.so.1", "libE.so.1", "libF.so.1"], &src);
        assert!(s.lib32.contains("libA.so.1") && !s.lib64.contains("libA.so.1"));
        assert!(s.lib64.contains("libB.so.1"));
        assert!(s.lib64.contains("libC.so.1") && s.lib32.contains("libC.so.1"));
        assert!(s.lib64.contains("libD.so.1") && !s.lib32.contains("libD.so.1"));
        assert!(!s.lib64.contains("libE.so.1"), "stale cache entry");
        assert!(!s.lib32.contains("libF.so.1"));
        assert!(resolve(&["libA.so.1"], &Sources::default()).is_empty());
    }

    #[test]
    fn parses_ld_so_conf_with_includes() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("ld.so.conf.d");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(tmp.path().join("ld.so.conf"), "# comment\ninclude ld.so.conf.d/*.conf\n/opt/a:/opt/b, /opt/c=libc6\nhwcap 0 nosegneg\n").unwrap();
        std::fs::write(d.join("10-lib32.conf"), "/usr/lib32\n").unwrap();
        std::fs::write(d.join("20-local.conf"), "/usr/local/lib # trailing\n").unwrap();
        std::fs::write(d.join("readme.txt"), "/not/included\n").unwrap();
        // Self-include ends at the depth limit.
        std::fs::write(d.join("30-loop.conf"), format!("include {}\n", d.join("30-loop.conf").display())).unwrap();
        let dirs = ld_so_conf_dirs(&tmp.path().join("ld.so.conf"));
        let want: Vec<PathBuf> = ["/usr/lib32", "/usr/local/lib", "/opt/a", "/opt/b", "/opt/c"].iter().map(PathBuf::from).collect();
        assert_eq!(dirs, want);
        assert!(ld_so_conf_dirs(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn splits_search_paths_and_globs() {
        assert_eq!(split_search_path("/a::/b;/c:"), [PathBuf::from("/a"), "/b".into(), "/c".into()]);
        assert!(wildcard("*.conf", "x.conf") && !wildcard("*.conf", "x.txt"));
        assert!(wildcard("lib?.so", "libz.so") && wildcard("*", "") && !wildcard("?", ""));
    }
}
