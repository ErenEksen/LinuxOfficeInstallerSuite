//! Live Office setup progress from the Click-to-Run logs (design D10,
//! office-installation: Live installation progress).
//!
//! Wine names C2R logs after the host, so every `.log` in the prefix's user temp folders
//! and in the configured log folder is followed. Files are read incrementally by byte
//! offset and decoded as UTF-16LE (UTF-8 when there is no BOM and no NUL bytes).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use regex::Regex;

#[derive(Default)]
struct Tail {
    offset: u64,
    utf16: Option<bool>,
    /// Undecoded trailing bytes (an odd byte or half a surrogate pair).
    bytes: Vec<u8>,
    /// Decoded text after the last newline.
    partial: String,
}

pub struct LogFollower {
    dirs: Vec<PathBuf>,
    since: SystemTime,
    tails: HashMap<PathBuf, Tail>,
    re: Regex,
    pub percent: Option<u32>,
}

impl LogFollower {
    /// Follow logs in the prefix's temp folders and `drive_c/<log_dir>`, ignoring files not
    /// modified since `since` (left over from earlier runs).
    pub fn for_prefix(prefix: &Path, log_dir: &str, since: SystemTime) -> Self {
        let mut dirs = vec![prefix.join("drive_c").join(log_dir)];
        if let Ok(users) = std::fs::read_dir(prefix.join("drive_c/users")) {
            for u in users.flatten() {
                dirs.push(u.path().join("AppData/Local/Temp"));
            }
        }
        Self::new(dirs, since)
    }

    pub fn new(dirs: Vec<PathBuf>, since: SystemTime) -> Self {
        Self {
            dirs,
            since,
            tails: HashMap::new(),
            re: Regex::new(r"total progress is now ([0-9]+)").unwrap(),
            percent: None,
        }
    }

    /// The most recently modified log being followed (shown when setup fails).
    pub fn latest_file(&self) -> Option<PathBuf> {
        self.tails
            .keys()
            .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok().map(|t| (t, p.clone())))
            .max()
            .map(|(_, p)| p)
    }

    /// Read whatever was appended since the last poll. Returns new complete lines; updates
    /// `percent` from the newest progress line.
    pub fn poll(&mut self) -> Vec<String> {
        let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
        for d in &self.dirs {
            let Ok(entries) = std::fs::read_dir(d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("log")) {
                    continue;
                }
                let Ok(meta) = e.metadata() else { continue };
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                if mtime >= self.since || self.tails.contains_key(&p) {
                    files.push((mtime, p));
                }
            }
        }
        // Oldest first, so the newest file's progress wins.
        files.sort();
        let mut lines = Vec::new();
        for (_, path) in files {
            let tail = self.tails.entry(path.clone()).or_default();
            let Ok(data) = read_from(&path, tail.offset) else { continue };
            if data.is_empty() {
                continue;
            }
            tail.offset += data.len() as u64;
            let new = feed(tail, &data);
            for l in &new {
                if let Some(c) = self.re.captures(l) {
                    self.percent = c[1].parse().ok().map(|p: u32| p.min(100));
                }
            }
            lines.extend(new);
        }
        lines
    }
}

fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    if len < offset {
        return Ok(Vec::new()); // Truncated or replaced; ignore.
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.take(4 << 20).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Decode a new chunk and return the complete lines it finishes.
fn feed(tail: &mut Tail, chunk: &[u8]) -> Vec<String> {
    tail.bytes.extend_from_slice(chunk);
    let utf16 = match tail.utf16 {
        Some(u) => u,
        None => {
            if tail.bytes.len() < 2 {
                return Vec::new();
            }
            // A BOM, or a NUL high byte after the first (ASCII) character.
            let u = tail.bytes.starts_with(&[0xFF, 0xFE]) || tail.bytes[1] == 0;
            if tail.bytes.starts_with(&[0xFF, 0xFE]) {
                tail.bytes.drain(..2);
            }
            tail.utf16 = Some(u);
            u
        }
    };
    let text = if utf16 {
        let bytes = std::mem::take(&mut tail.bytes);
        let even = bytes.len() & !1;
        let mut units: Vec<u16> = bytes[..even].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let mut keep = bytes[even..].to_vec();
        // Hold back a trailing high surrogate until its pair arrives.
        if let Some(&last) = units.last()
            && (0xD800..0xDC00).contains(&last)
        {
            units.pop();
            let mut k = last.to_le_bytes().to_vec();
            k.extend(keep);
            keep = k;
        }
        tail.bytes = keep;
        String::from_utf16_lossy(&units)
    } else {
        let bytes = std::mem::take(&mut tail.bytes);
        match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(e) if e.error_len().is_none() => {
                let (ok, rest) = bytes.split_at(e.valid_up_to());
                tail.bytes = rest.to_vec();
                String::from_utf8_lossy(ok).into_owned()
            }
            Err(_) => String::from_utf8_lossy(&bytes).into_owned(),
        }
    };
    tail.partial.push_str(&text);
    let mut lines = Vec::new();
    while let Some(i) = tail.partial.find('\n') {
        let line: String = tail.partial.drain(..=i).collect();
        let line = line.trim_end_matches(['\n', '\r']).to_string();
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { vec![0xFF, 0xFE] } else { vec![] };
        v.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        v
    }

    const SAMPLE: &str = "10/19/2026 14:01:02.123\tOfficeClickToRun.exe (0x1a4)\tScenarioController::UpdateScenarioProgress - total progress is now 3\r\n\
                          10/19/2026 14:01:09.001\tOfficeClickToRun.exe (0x1a4)\tStreaming file \u{1F4C4}\r\n\
                          10/19/2026 14:02:44.500\tOfficeClickToRun.exe (0x1a4)\tScenarioController::UpdateScenarioProgress - total progress is now 42\r\n";

    #[test]
    fn decodes_utf16_split_at_every_boundary() {
        let bytes = utf16(SAMPLE, true);
        // Every split point, including odd bytes and the middle of the surrogate pair.
        for cut in 1..bytes.len() {
            let mut t = Tail::default();
            t.offset = cut as u64;
            let mut lines = feed(&mut t, &bytes[..cut]);
            t.offset = bytes.len() as u64;
            lines.extend(feed(&mut t, &bytes[cut..]));
            assert_eq!(lines.len(), 3, "cut {cut}");
            assert!(lines[1].ends_with("Streaming file \u{1F4C4}"), "cut {cut}: {:?}", lines[1]);
            assert!(!lines[0].starts_with('\u{FEFF}'));
        }
    }

    #[test]
    fn follows_growing_files_and_reports_percent() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path();
        let temp = prefix.join("drive_c/users/axis/AppData/Local/Temp");
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::create_dir_all(prefix.join("drive_c/msoffice-logs")).unwrap();
        let since = SystemTime::now() - std::time::Duration::from_secs(1);
        let mut f = LogFollower::for_prefix(prefix, "msoffice-logs", since);
        assert!(f.poll().is_empty());
        assert_eq!(f.percent, None, "indeterminate until a percentage appears");

        let path = temp.join("MYHOST-20261019-1401.log");
        let all = utf16(SAMPLE, true);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&all[..101]).unwrap(); // odd split mid-line
        file.flush().unwrap();
        let first = f.poll();
        assert_eq!(first.len(), 0);
        file.write_all(&all[101..]).unwrap();
        file.flush().unwrap();
        let lines = f.poll();
        assert_eq!(lines.len(), 3);
        assert_eq!(f.percent, Some(42));

        // A plain UTF-8 log in the logging folder is followed too.
        std::fs::write(prefix.join("drive_c/msoffice-logs/setup.log"), "x total progress is now 77\n").unwrap();
        f.poll();
        assert_eq!(f.percent, Some(77));
    }
}
