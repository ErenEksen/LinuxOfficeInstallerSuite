//! Resumable HTTP downloads with checksum verification.
//!
//! Data goes to `<dest>.part`; an interrupted download resumes with a `Range` request.
//! The file is renamed to `<dest>` only after every expected checksum matches. A mismatch
//! deletes the download.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use md5::Md5;
use reqwest::StatusCode;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_RANGE, RANGE};
use sha2::{Digest, Sha256};

use super::Cancel;

pub const USER_AGENT: &str = concat!("LinuxOfficeInstallerSuite/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Checksum {
    Sha256(String),
    Md5(String),
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download of {url} failed: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("download of {url} failed: server answered {status}")]
    Status { url: String, status: StatusCode },
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Mismatch { file: String, expected: String, actual: String },
    #[error("{url} returned a web page instead of the file; the download site may have changed")]
    WebPage { url: String },
    #[error("download cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] io::Error),
}

pub fn http_client() -> Client {
    http_client_with_timeout(Duration::from_secs(60))
}

/// `timeout` applies to each connect/read/write operation of the blocking client, not to the
/// whole transfer: the ~1 GB Win7 SP1 package is fine while a stalled connection still fails.
pub fn http_client_with_timeout(timeout: Duration) -> Client {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(20))
        .timeout(timeout)
        .build()
        .expect("build HTTP client")
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Compute the checksum of the same kind as `expected` and compare.
pub fn check_file(path: &Path, expected: &[Checksum]) -> Result<(), DownloadError> {
    for want in expected {
        let (want_hex, got) = match want {
            Checksum::Sha256(h) => (h, hash_file::<Sha256>(path)?),
            Checksum::Md5(h) => (h, hash_file::<Md5>(path)?),
        };
        if !want_hex.eq_ignore_ascii_case(&got) {
            return Err(DownloadError::Mismatch {
                file: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                expected: want_hex.to_lowercase(),
                actual: got,
            });
        }
    }
    Ok(())
}

pub fn hash_file<D: Digest>(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Download `url` to `dest`, verifying `expected` checksums.
///
/// If `dest` already exists and matches, nothing is downloaded (cache hit). `progress`
/// receives `(bytes_done, total_bytes_if_known)`.
pub fn download(
    client: &Client,
    url: &str,
    dest: &Path,
    expected: &[Checksum],
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &Cancel,
) -> Result<(), DownloadError> {
    if dest.exists() {
        if check_file(dest, expected).is_ok() {
            let len = dest.metadata()?.len();
            progress(len, Some(len));
            return Ok(());
        }
        log::info!("cached {} does not match, downloading again", dest.display());
        std::fs::remove_file(dest)?;
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);

    // One retry from scratch if a resumed .part turns out to be bad.
    for attempt in 0..2 {
        fetch_to_part(client, url, &part, progress, cancel)?;
        match check_file(&part, expected) {
            Ok(()) => {
                std::fs::rename(&part, dest)?;
                return Ok(());
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                if attempt == 1 || !matches!(e, DownloadError::Mismatch { .. }) {
                    // Report the final file name, not the .part name.
                    return Err(match e {
                        DownloadError::Mismatch { expected, actual, .. } => DownloadError::Mismatch {
                            file: dest.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                            expected,
                            actual,
                        },
                        other => other,
                    });
                }
                log::warn!("checksum mismatch on resumed download of {url}, retrying from scratch");
            }
        }
    }
    unreachable!()
}

fn fetch_to_part(
    client: &Client,
    url: &str,
    part: &Path,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &Cancel,
) -> Result<(), DownloadError> {
    let have = part.metadata().map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().map_err(|source| DownloadError::Http { url: url.into(), source })?;
    let status = resp.status();
    // Download sites answer some clients with an HTML landing page; never save it as the file.
    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
    if is_html && status.is_success() {
        return Err(DownloadError::WebPage { url: url.into() });
    }

    let (mut file, mut done, total) = match status {
        StatusCode::PARTIAL_CONTENT if have > 0 => {
            let total = resp
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|t| t.parse::<u64>().ok())
                .or_else(|| resp.content_length().map(|l| l + have));
            (OpenOptions::new().append(true).open(part)?, have, total)
        }
        // The .part already holds the whole file.
        StatusCode::RANGE_NOT_SATISFIABLE if have > 0 => {
            progress(have, Some(have));
            return Ok(());
        }
        s if s.is_success() => (File::create(part)?, 0, resp.content_length()),
        s => return Err(DownloadError::Status { url: url.into(), status: s }),
    };

    progress(done, total);
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.is_cancelled() {
            file.flush()?;
            return Err(DownloadError::Cancelled);
        }
        let n = resp.read(&mut buf).map_err(|e| {
            // Keep the .part: the next attempt resumes from here.
            DownloadError::Io(io::Error::new(e.kind(), format!("download of {url} interrupted: {e}")))
        })?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    file.flush()?;
    if let Some(t) = total
        && done < t
    {
        return Err(DownloadError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("download of {url} ended early ({done} of {t} bytes)"),
        )));
    }
    Ok(())
}

/// Size and validator of a remote file, from a `HEAD` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFile {
    pub size: u64,
    pub etag: Option<String>,
}

/// `HEAD url`: `Ok(None)` for 404, an error for any other failure.
pub fn head(client: &Client, url: &str) -> Result<Option<RemoteFile>, DownloadError> {
    let resp = client.head(url).send().map_err(|source| DownloadError::Http { url: url.into(), source })?;
    match resp.status() {
        StatusCode::NOT_FOUND => Ok(None),
        s if s.is_success() => {
            let size = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| {
                    DownloadError::Io(io::Error::other(format!("{url} did not report its size")))
                })?;
            let etag = resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_string);
            Ok(Some(RemoteFile { size, etag }))
        }
        s => Err(DownloadError::Status { url: url.into(), status: s }),
    }
}

/// How [`download_ranged`] splits a large file.
#[derive(Clone, Copy, Debug)]
pub struct RangedOpts {
    pub connections: usize,
    pub chunk: u64,
    /// Attempts per chunk before the whole download fails.
    pub attempts: u32,
    pub retry_delay: Duration,
}

impl Default for RangedOpts {
    fn default() -> Self {
        Self { connections: 6, chunk: 16 << 20, attempts: 6, retry_delay: Duration::from_secs(2) }
    }
}

fn chunks_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part.chunks");
    dest.with_file_name(name)
}

/// Finished chunk indices recorded in `<dest>.part.chunks`, if it belongs to the same remote
/// file (first line `<size> <etag>`).
fn read_done_chunks(state: &Path, header: &str) -> Option<std::collections::HashSet<u64>> {
    let text = std::fs::read_to_string(state).ok()?;
    let mut lines = text.lines();
    if lines.next()? != header {
        return None;
    }
    Some(lines.filter_map(|l| l.trim().parse().ok()).collect())
}

/// Download a file of known `remote.size` over several parallel `Range` connections.
///
/// Chunks are written in place into `<dest>.part`; each finished chunk is recorded in
/// `<dest>.part.chunks`, so an interrupted or cancelled download resumes with only the
/// missing chunks. A failed chunk is retried `opts.attempts` times. `<dest>` appears only
/// once every byte is there. If `dest` already has the right size it is a cache hit.
pub fn download_ranged(
    client: &Client,
    url: &str,
    dest: &Path,
    remote: &RemoteFile,
    opts: RangedOpts,
    progress: &mut dyn FnMut(u64, Option<u64>),
    cancel: &Cancel,
) -> Result<(), DownloadError> {
    use std::collections::VecDeque;
    use std::os::unix::fs::FileExt;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let size = remote.size;
    if dest.metadata().is_ok_and(|m| m.len() == size) {
        progress(size, Some(size));
        return Ok(());
    }
    let _ = std::fs::remove_file(dest);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let state = chunks_path(dest);
    let header = format!("{size} {}", remote.etag.as_deref().unwrap_or("-"));
    let chunk = opts.chunk.max(1);
    let count = size.div_ceil(chunk);
    let span = |i: u64| (i * chunk, ((i + 1) * chunk).min(size));

    let done_chunks = match read_done_chunks(&state, &header) {
        Some(d) if part.metadata().is_ok_and(|m| m.len() == size) => d,
        _ => {
            let _ = std::fs::remove_file(&part);
            std::fs::write(&state, format!("{header}\n"))?;
            Default::default()
        }
    };
    let file = OpenOptions::new().create(true).write(true).truncate(false).open(&part)?;
    file.set_len(size)?;
    let pending: VecDeque<u64> = (0..count).filter(|i| !done_chunks.contains(i)).collect();
    let have: u64 = done_chunks.iter().filter(|i| **i < count).map(|i| span(*i).1 - span(*i).0).sum();

    let queue = Mutex::new(pending);
    let state_file = Mutex::new(OpenOptions::new().append(true).open(&state)?);
    let done = AtomicU64::new(have);
    let failed = AtomicBool::new(false);
    let error: Mutex<Option<DownloadError>> = Mutex::new(None);
    let workers = opts.connections.clamp(1, queue.lock().unwrap().len().max(1));

    let fetch_chunk = |i: u64| -> Result<(), DownloadError> {
        let (start, end) = span(i);
        let resp = client
            .get(url)
            .header(RANGE, format!("bytes={start}-{}", end - 1))
            .send()
            .map_err(|source| DownloadError::Http { url: url.into(), source })?;
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(DownloadError::Status { url: url.into(), status: resp.status() });
        }
        let mut resp = resp;
        let mut pos = start;
        let mut buf = vec![0u8; 1 << 16];
        let result = loop {
            if cancel.is_cancelled() || failed.load(Ordering::SeqCst) {
                break Err(DownloadError::Cancelled);
            }
            let want = ((end - pos) as usize).min(buf.len());
            if want == 0 {
                break Ok(());
            }
            match resp.read(&mut buf[..want]) {
                Ok(0) => {
                    break Err(DownloadError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("download of {url} ended early (bytes {start}-{end}, got up to {pos})"),
                    )));
                }
                Ok(n) => {
                    if let Err(e) = file.write_all_at(&buf[..n], pos) {
                        break Err(e.into());
                    }
                    pos += n as u64;
                    done.fetch_add(n as u64, Ordering::Relaxed);
                }
                Err(e) => {
                    break Err(DownloadError::Io(io::Error::new(e.kind(), format!("download of {url} interrupted: {e}"))));
                }
            }
        };
        if result.is_err() {
            // The chunk starts over on the next attempt.
            done.fetch_sub(pos - start, Ordering::Relaxed);
        }
        result
    };

    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(s.spawn(|| {
                loop {
                    if failed.load(Ordering::SeqCst) || cancel.is_cancelled() {
                        return;
                    }
                    let Some(i) = queue.lock().unwrap().pop_front() else { return };
                    let mut attempt = 0;
                    loop {
                        attempt += 1;
                        match fetch_chunk(i) {
                            Ok(()) => {
                                let mut f = state_file.lock().unwrap();
                                let _ = writeln!(f, "{i}");
                                break;
                            }
                            Err(DownloadError::Cancelled) => return,
                            Err(e) if attempt >= opts.attempts => {
                                failed.store(true, Ordering::SeqCst);
                                error.lock().unwrap().get_or_insert(e);
                                return;
                            }
                            Err(e) => {
                                log::warn!("chunk {i} of {url}, attempt {attempt}: {e}; retrying");
                                let wait = opts.retry_delay * attempt;
                                let until = std::time::Instant::now() + wait;
                                while std::time::Instant::now() < until {
                                    if cancel.is_cancelled() || failed.load(Ordering::SeqCst) {
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(100));
                                }
                            }
                        }
                    }
                }
            }));
        }
        // The progress callback is not `Send`; report from this thread while the workers run.
        while !handles.iter().all(|h| h.is_finished()) {
            progress(done.load(Ordering::Relaxed), Some(size));
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    file.sync_all()?;
    drop(file);
    if let Some(e) = error.into_inner().unwrap() {
        return Err(e);
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let finished = read_done_chunks(&state, &header).unwrap_or_default();
    if (0..count).any(|i| !finished.contains(&i)) {
        return Err(DownloadError::Io(io::Error::other(format!("download of {url} is incomplete"))));
    }
    std::fs::rename(&part, dest)?;
    let _ = std::fs::remove_file(&state);
    progress(size, Some(size));
    Ok(())
}

/// `1.5 MB`, `947.1 MB`, `2.3 GB` (decimal units, like download sites).
pub fn format_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1e9 {
        format!("{:.1} GB", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1} MB", n / 1e6)
    } else {
        format!("{:.0} KB", n / 1e3)
    }
}

/// Turns raw `(done, total)` callbacks into human-readable status: a short live line at
/// most once a second, and a log line every 10 %.
pub struct Meter {
    start: std::time::Instant,
    start_bytes: Option<u64>,
    last_live: Option<std::time::Instant>,
    last_logged_tenth: Option<u64>,
}

#[derive(Debug, Default, PartialEq)]
pub struct MeterUpdate {
    /// e.g. `123.4 MB / 947.1 MB · 0.4 MB/s` (throttled to once a second).
    pub live: Option<String>,
    /// e.g. `20 % (189.4 MB / 947.1 MB, 0.4 MB/s)` (every 10 %).
    pub log: Option<String>,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    pub fn new() -> Self {
        Self { start: std::time::Instant::now(), start_bytes: None, last_live: None, last_logged_tenth: None }
    }

    fn speed(&self, done: u64) -> Option<String> {
        let secs = self.start.elapsed().as_secs_f64();
        let got = done.saturating_sub(self.start_bytes.unwrap_or(done));
        (secs >= 1.0 && got > 0).then(|| format!("{}/s", format_bytes((got as f64 / secs) as u64)))
    }

    pub fn update(&mut self, done: u64, total: Option<u64>) -> MeterUpdate {
        // Bytes already on disk from an earlier attempt do not count towards the speed.
        self.start_bytes.get_or_insert(done);
        let mut out = MeterUpdate::default();
        let now = std::time::Instant::now();
        let finished = total.is_some_and(|t| done >= t);
        if finished || self.last_live.is_none_or(|t| now.duration_since(t).as_millis() >= 1000) {
            self.last_live = Some(now);
            let mut s = match total {
                Some(t) => format!("{} / {}", format_bytes(done), format_bytes(t)),
                None => format_bytes(done),
            };
            if let Some(sp) = self.speed(done) {
                s.push_str(&format!(" · {sp}"));
            }
            out.live = Some(s);
        }
        if let Some(t) = total.filter(|t| *t > 0) {
            let tenth = done.min(t) * 10 / t;
            if self.last_logged_tenth.is_none_or(|l| tenth > l) {
                self.last_logged_tenth = Some(tenth);
                let mut s = format!("{} % ({} / {}", tenth * 10, format_bytes(done), format_bytes(t));
                if let Some(sp) = self.speed(done) {
                    s.push_str(&format!(", {sp}"));
                }
                s.push(')');
                out.log = Some(s);
            }
        }
        out
    }
}

/// Fetch a small text resource (API responses, checksum lists).
pub fn get_text(client: &Client, url: &str) -> Result<String, DownloadError> {
    let resp = client.get(url).send().map_err(|source| DownloadError::Http { url: url.into(), source })?;
    if !resp.status().is_success() {
        return Err(DownloadError::Status { url: url.into(), status: resp.status() });
    }
    resp.text().map_err(|source| DownloadError::Http { url: url.into(), source })
}

/// A tiny HTTP/1.1 server for tests: serves fixed bodies, honours `Range`, and can drop the
/// connection after N bytes to simulate an interrupted download.
#[cfg(test)]
pub mod testserver {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct Route {
        pub body: Vec<u8>,
        pub status: u16,
        pub extra_headers: Vec<(String, String)>,
        /// Close the connection after this many body bytes (simulated network drop).
        pub cut_after: Option<usize>,
        /// After `cut_after` bytes, keep the connection open and silent for this long.
        pub stall: Option<std::time::Duration>,
    }

    impl Route {
        pub fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self { body: body.into(), status: 200, extra_headers: vec![], cut_after: None, stall: None }
        }
    }

    pub struct Server {
        pub base: String,
        pub routes: Arc<Mutex<HashMap<String, Route>>>,
        /// Raw request heads, in arrival order.
        pub requests: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        pub fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let routes: Arc<Mutex<HashMap<String, Route>>> = Default::default();
            let requests: Arc<Mutex<Vec<String>>> = Default::default();
            let (r, q) = (routes.clone(), requests.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let (r, q) = (r.clone(), q.clone());
                    // One thread per connection, so parallel range downloads really overlap.
                    std::thread::spawn(move || serve(stream, &r, &q));
                }
            });
            Self { base, routes, requests }
        }

        pub fn route(&self, path: &str, route: Route) -> String {
            self.routes.lock().unwrap().insert(path.to_string(), route);
            format!("{}{path}", self.base)
        }
    }

    fn serve(mut stream: std::net::TcpStream, routes: &Mutex<HashMap<String, Route>>, requests: &Mutex<Vec<String>>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        requests.lock().unwrap().push(head.clone());
        let is_head = head.starts_with("HEAD ");
        let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        // `bytes=S-` or `bytes=S-E`.
        let range = head
            .lines()
            .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|v| v.trim().to_string()))
            .and_then(|v| {
                let (s, e) = v.split_once('-')?;
                Some((s.parse::<usize>().ok()?, e.parse::<usize>().ok()))
            });
        let route = routes.lock().unwrap().get(&path).cloned();
        let Some(route) = route else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            return;
        };
        let len = route.body.len();
        let (status, body, mut headers) = match range {
            Some((s, _)) if route.status == 200 && s >= len => {
                (416, Vec::new(), vec![format!("Content-Range: bytes */{len}")])
            }
            Some((s, e)) if route.status == 200 => {
                let e = e.map(|e| e.min(len - 1)).unwrap_or(len - 1);
                (206, route.body[s..=e].to_vec(), vec![format!("Content-Range: bytes {s}-{e}/{len}")])
            }
            _ => (route.status, route.body.clone(), vec![]),
        };
        headers.extend(route.extra_headers.iter().map(|(k, v)| format!("{k}: {v}")));
        let mut resp = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        for h in headers {
            resp.push_str(&h);
            resp.push_str("\r\n");
        }
        resp.push_str("\r\n");
        let _ = stream.write_all(resp.as_bytes());
        if is_head {
            return;
        }
        let cut = route.cut_after.map(|c| c.min(body.len())).unwrap_or(body.len());
        let _ = stream.write_all(&body[..cut]);
        let _ = stream.flush();
        if let Some(d) = route.stall {
            std::thread::sleep(d);
        }
        // Dropping the stream closes the connection.
    }
}

#[cfg(test)]
mod tests {
    use super::testserver::{Route, Server};
    use super::*;

    fn body() -> Vec<u8> {
        (0..300_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    fn md5(bytes: &[u8]) -> String {
        hex::encode(Md5::digest(bytes))
    }

    #[test]
    fn downloads_and_verifies_both_hashes() {
        let srv = Server::start();
        let data = body();
        let url = srv.route("/a.bin", Route::ok(data.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("a.bin");
        let mut last = (0, None);
        download(
            &http_client(),
            &url,
            &dest,
            &[Checksum::Sha256(sha(&data)), Checksum::Md5(md5(&data).to_uppercase())],
            &mut |d, t| last = (d, t),
            &Cancel::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(last, (data.len() as u64, Some(data.len() as u64)));
        assert!(!part_path(&dest).exists());

        // Second call is a cache hit: no new request.
        let before = srv.requests.lock().unwrap().len();
        download(&http_client(), &url, &dest, &[Checksum::Sha256(sha(&data))], &mut |_, _| {}, &Cancel::new())
            .unwrap();
        assert_eq!(srv.requests.lock().unwrap().len(), before);
    }

    #[test]
    fn resumes_an_interrupted_download_with_range() {
        let srv = Server::start();
        let data = body();
        let mut route = Route::ok(data.clone());
        route.cut_after = Some(100_000);
        let url = srv.route("/r.bin", route);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("r.bin");
        let checks = [Checksum::Sha256(sha(&data))];

        let err = download(&http_client(), &url, &dest, &checks, &mut |_, _| {}, &Cancel::new()).unwrap_err();
        assert!(matches!(err, DownloadError::Io(_)), "{err:?}");
        let have = part_path(&dest).metadata().unwrap().len();
        assert!(have > 0 && have <= 100_000, "partial bytes kept: {have}");

        srv.route("/r.bin", Route::ok(data.clone()));
        download(&http_client(), &url, &dest, &checks, &mut |_, _| {}, &Cancel::new()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let reqs = srv.requests.lock().unwrap();
        assert!(reqs.last().unwrap().to_ascii_lowercase().contains(&format!("range: bytes={have}-")));
    }

    #[test]
    fn mismatch_deletes_the_file() {
        let srv = Server::start();
        let url = srv.route("/m.bin", Route::ok(body()));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("m.bin");
        let err = download(
            &http_client(),
            &url,
            &dest,
            &[Checksum::Sha256("00".repeat(32))],
            &mut |_, _| {},
            &Cancel::new(),
        )
        .unwrap_err();
        match err {
            DownloadError::Mismatch { file, .. } => assert_eq!(file, "m.bin"),
            e => panic!("unexpected {e:?}"),
        }
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[test]
    fn cancel_stops_and_keeps_the_part() {
        let srv = Server::start();
        let data = body();
        let url = srv.route("/c.bin", Route::ok(data.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("c.bin");
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let err = download(
            &http_client(),
            &url,
            &dest,
            &[Checksum::Sha256(sha(&data))],
            &mut |d, _| {
                if d > 0 {
                    c2.cancel()
                }
            },
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
        assert!(!dest.exists());
        assert!(part_path(&dest).exists());
    }

    #[test]
    fn stalled_connection_times_out() {
        let srv = Server::start();
        let mut route = Route::ok(body());
        route.cut_after = Some(50_000);
        route.stall = Some(Duration::from_secs(20));
        let url = srv.route("/stall.bin", route);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("stall.bin");
        let start = std::time::Instant::now();
        let err = download(&http_client_with_timeout(Duration::from_secs(1)), &url, &dest, &[], &mut |_, _| {}, &Cancel::new())
            .unwrap_err();
        eprintln!("stall error after {:?}: {err}", start.elapsed());
        assert!(start.elapsed() < Duration::from_secs(10), "took {:?}", start.elapsed());
        assert!(part_path(&dest).exists(), "partial data kept for resume");
    }

    fn ranged_opts() -> RangedOpts {
        RangedOpts { connections: 4, chunk: 32_768, attempts: 2, retry_delay: Duration::from_millis(10) }
    }

    #[test]
    fn head_reports_size_and_missing_files() {
        let srv = Server::start();
        let mut route = Route::ok(body());
        route.extra_headers = vec![("ETag".into(), "\"0xABC\"".into())];
        let url = srv.route("/h.bin", route);
        let r = head(&http_client(), &url).unwrap().unwrap();
        assert_eq!(r, RemoteFile { size: 300_000, etag: Some("\"0xABC\"".into()) });
        assert_eq!(head(&http_client(), &format!("{}/nope", srv.base)).unwrap(), None);
    }

    #[test]
    fn ranged_download_uses_parallel_ranges() {
        let srv = Server::start();
        let data = body();
        let url = srv.route("/big.bin", Route::ok(data.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("big.bin");
        let remote = RemoteFile { size: data.len() as u64, etag: None };
        let mut last = 0;
        download_ranged(&http_client(), &url, &dest, &remote, ranged_opts(), &mut |d, _| last = d, &Cancel::new())
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(last, data.len() as u64);
        assert!(!part_path(&dest).exists() && !chunks_path(&dest).exists());
        let ranges = srv.requests.lock().unwrap().iter().filter(|r| r.to_ascii_lowercase().contains("range: bytes=")).count();
        assert_eq!(ranges, data.len().div_ceil(32_768), "one request per chunk");

        // Cache hit: no new request.
        let before = srv.requests.lock().unwrap().len();
        download_ranged(&http_client(), &url, &dest, &remote, ranged_opts(), &mut |_, _| {}, &Cancel::new()).unwrap();
        assert_eq!(srv.requests.lock().unwrap().len(), before);
    }

    #[test]
    fn ranged_download_resumes_only_missing_chunks() {
        let srv = Server::start();
        let data = body();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("res.bin");
        let remote = RemoteFile { size: data.len() as u64, etag: Some("e1".into()) };

        // Every response drops after 40 000 bytes: chunks of 32 768 succeed, the short last
        // chunk too, but a chunk size above the cut always fails.
        let mut route = Route::ok(data.clone());
        route.cut_after = Some(40_000);
        let url = srv.route("/res.bin", route);
        let opts = RangedOpts { connections: 5, chunk: 65_536, ..ranged_opts() };
        let err = download_ranged(&http_client(), &url, &dest, &remote, opts, &mut |_, _| {}, &Cancel::new()).unwrap_err();
        assert!(matches!(err, DownloadError::Io(_) | DownloadError::Http { .. }), "{err:?}");
        assert!(!dest.exists());
        let kept = read_done_chunks(&chunks_path(&dest), "300000 e1").unwrap();
        // Chunk 4 (the short last one) is the only one that can finish; it usually does.
        assert!(kept.iter().all(|i| *i == 4), "{kept:?}");

        srv.route("/res.bin", Route::ok(data.clone()));
        let before = srv.requests.lock().unwrap().len();
        download_ranged(&http_client(), &url, &dest, &remote, opts, &mut |_, _| {}, &Cancel::new()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let reqs = srv.requests.lock().unwrap();
        assert_eq!(reqs.len() - before, 5 - kept.len(), "finished chunks are not fetched again");
        if kept.contains(&4) {
            assert!(!reqs[before..].iter().any(|r| r.contains("bytes=262144-")));
        }
    }

    #[test]
    fn ranged_download_restarts_when_the_remote_file_changed() {
        let srv = Server::start();
        let data = body();
        let url = srv.route("/chg.bin", Route::ok(data.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("chg.bin");
        // A leftover state for another ETag must be ignored.
        std::fs::write(part_path(&dest), vec![0u8; data.len()]).unwrap();
        std::fs::write(chunks_path(&dest), "300000 old\n0\n1\n2\n").unwrap();
        let remote = RemoteFile { size: data.len() as u64, etag: Some("new".into()) };
        download_ranged(&http_client(), &url, &dest, &remote, ranged_opts(), &mut |_, _| {}, &Cancel::new()).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn ranged_download_cancel_keeps_progress() {
        let srv = Server::start();
        let data = body();
        // Each response sends a little, then stalls: the download is still running when the
        // progress callback cancels it.
        let mut route = Route::ok(data.clone());
        route.cut_after = Some(10_000);
        route.stall = Some(Duration::from_secs(2));
        let url = srv.route("/cr.bin", route);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("cr.bin");
        let remote = RemoteFile { size: data.len() as u64, etag: None };
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let err = download_ranged(
            &http_client(),
            &url,
            &dest,
            &remote,
            RangedOpts { connections: 1, ..ranged_opts() },
            &mut |d, _| {
                if d > 0 {
                    c2.cancel()
                }
            },
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled), "{err:?}");
        assert!(!dest.exists());
        assert!(part_path(&dest).exists() && chunks_path(&dest).exists());
    }

    #[test]
    fn meter_logs_every_ten_percent() {
        let mut m = Meter::new();
        let total = 1_000_000_000;
        let logs: Vec<String> = (0..=100u64).filter_map(|p| m.update(p * total / 100, Some(total)).log).collect();
        assert_eq!(logs.len(), 11, "{logs:?}");
        assert!(logs[0].starts_with("0 % (0 KB / 1.0 GB"));
        assert!(logs[10].starts_with("100 % (1.0 GB / 1.0 GB"));
        assert_eq!(format_bytes(947_070_088), "947.1 MB");
        assert_eq!(format_bytes(554_208), "554 KB");
        let mut m = Meter::new();
        assert_eq!(m.update(5, Some(10)).live.as_deref(), Some("0 KB / 0 KB"));
        assert!(m.update(6, Some(10)).live.is_none(), "throttled to once a second");
        assert!(m.update(10, Some(10)).live.is_some(), "the final update is always shown");
    }

    #[test]
    fn html_landing_page_is_rejected() {
        let srv = Server::start();
        let mut route = Route::ok("<!doctype html><html>download will start</html>");
        route.extra_headers = vec![("Content-Type".into(), "text/html; charset=utf-8".into())];
        let url = srv.route("/font.exe", route);
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("font.exe");
        let err = download(&http_client(), &url, &dest, &[Checksum::Md5("00".repeat(16))], &mut |_, _| {}, &Cancel::new())
            .unwrap_err();
        assert!(matches!(err, DownloadError::WebPage { .. }), "{err:?}");
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[test]
    fn http_error_status_is_reported() {
        let srv = Server::start();
        let tmp = tempfile::tempdir().unwrap();
        let err = download(
            &http_client(),
            &format!("{}/missing", srv.base),
            &tmp.path().join("x"),
            &[],
            &mut |_, _| {},
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(err, DownloadError::Status { status, .. } if status == StatusCode::NOT_FOUND));
    }

    /// Diagnose what a URL serves to this client: `LOIS_URL=… cargo test -- --ignored probe_url --nocapture`.
    #[test]
    #[ignore]
    fn probe_url() {
        let url = std::env::var("LOIS_URL").unwrap();
        let resp = http_client().get(&url).send().unwrap();
        eprintln!("final url: {}\nstatus: {}\ncontent-type: {:?}", resp.url(), resp.status(), resp.headers().get("content-type"));
        let body = resp.bytes().unwrap();
        eprintln!("bytes: {} md5: {}", body.len(), hex::encode(Md5::digest(&body)));
        eprintln!("head: {}", String::from_utf8_lossy(&body[..body.len().min(300)]));
    }
}
