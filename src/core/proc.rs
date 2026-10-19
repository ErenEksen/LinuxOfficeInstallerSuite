//! Child processes with streamed output, per-operation log files and cancellation
//! (app-settings: Operation logs).

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::Cancel;
use super::time::{clock, file_stamp};

/// A timestamped log file in `~/.msoffice/logs/`, shared by every step of one operation.
#[derive(Clone)]
pub struct OpLog {
    path: PathBuf,
    file: Arc<Mutex<File>>,
}

impl OpLog {
    /// Create `<logs>/<stamp>-<operation>.log`.
    pub fn create(logs_dir: &Path, operation: &str) -> io::Result<Self> {
        std::fs::create_dir_all(logs_dir)?;
        let stamp = file_stamp(SystemTime::now());
        let mut path = logs_dir.join(format!("{stamp}-{operation}.log"));
        let mut n = 1;
        while path.exists() {
            n += 1;
            path = logs_dir.join(format!("{stamp}-{operation}-{n}.log"));
        }
        let file = File::create(&path)?;
        Ok(Self { path, file: Arc::new(Mutex::new(file)) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn line(&self, text: &str) {
        let mut f = self.file.lock().unwrap();
        let _ = writeln!(f, "{} {}", clock(SystemTime::now()), text);
        let _ = f.flush();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    #[error("could not start {program}: {source}")]
    Spawn { program: String, source: io::Error },
    #[error("{program} exited with {status}")]
    Failed { program: String, status: String, code: Option<i32> },
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// A command description; built up then run with [`run`].
#[derive(Clone, Debug)]
pub struct Cmd {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub env_remove: Vec<OsString>,
    pub cwd: Option<PathBuf>,
}

impl Cmd {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self { program: program.into(), args: vec![], env: vec![], env_remove: vec![], cwd: None }
    }
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }
    pub fn args<I: IntoIterator<Item = S>, S: Into<OsString>>(mut self, it: I) -> Self {
        self.args.extend(it.into_iter().map(Into::into));
        self
    }
    pub fn env(mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }
    pub fn env_remove(mut self, k: impl Into<OsString>) -> Self {
        self.env_remove.push(k.into());
        self
    }

    pub fn display(&self) -> String {
        std::iter::once(&self.program)
            .chain(&self.args)
            .map(|s| {
                let s = s.to_string_lossy();
                if s.is_empty() || s.contains(|c: char| c.is_whitespace() || "'\"\\$`".contains(c)) {
                    format!("'{}'", s.replace('\'', "'\\''"))
                } else {
                    s.into_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.args);
        for k in &self.env_remove {
            c.env_remove(k);
        }
        for (k, v) in &self.env {
            c.env(k, v);
        }
        if let Some(d) = &self.cwd {
            c.current_dir(d);
        }
        c
    }
}

/// Outcome of a finished process.
#[derive(Debug)]
pub struct Output {
    pub status: ExitStatus,
    /// All output lines (stdout and stderr interleaved), for error reports.
    pub lines: Vec<String>,
}

impl Output {
    pub fn tail(&self, n: usize) -> String {
        let start = self.lines.len().saturating_sub(n);
        self.lines[start..].join("\n")
    }
}

fn program_name(cmd: &Cmd) -> String {
    Path::new(&cmd.program).file_name().unwrap_or(cmd.program.as_os_str()).to_string_lossy().into_owned()
}

/// Run `cmd` in its own process group. Every stdout/stderr line goes to `on_line` and the
/// log. Cancelling kills the whole process group. Returns the output even for a non-zero
/// exit; use [`run_checked`] to turn that into an error.
pub fn run(
    cmd: &Cmd,
    log: Option<&OpLog>,
    on_line: &mut dyn FnMut(Stream, &str),
    cancel: &Cancel,
) -> Result<Output, ProcError> {
    if let Some(l) = log {
        l.line(&format!("$ {}", cmd.display()));
    }
    let mut command = cmd.command();
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = command.spawn().map_err(|source| ProcError::Spawn { program: program_name(cmd), source })?;
    let pgid = child.id() as i32;

    let (tx, rx) = mpsc::channel::<(Stream, String)>();
    let readers = [
        spawn_reader(child.stdout.take().unwrap(), Stream::Stdout, tx.clone()),
        spawn_reader(child.stderr.take().unwrap(), Stream::Stderr, tx),
    ];

    let mut lines = Vec::new();
    let mut cancelled_at: Option<Instant> = None;
    let status = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok((stream, line)) => {
                if let Some(l) = log {
                    l.line(&line);
                }
                on_line(stream, &line);
                lines.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Both pipes closed: wait for the process itself.
                if cancelled_at.is_none() && cancel.is_cancelled() {
                    kill_group(pgid, libc::SIGTERM);
                    cancelled_at = Some(Instant::now());
                }
                break child.wait()?;
            }
        }
        if cancel.is_cancelled() {
            match cancelled_at {
                None => {
                    kill_group(pgid, libc::SIGTERM);
                    cancelled_at = Some(Instant::now());
                }
                Some(t) if t.elapsed() > Duration::from_secs(3) => kill_group(pgid, libc::SIGKILL),
                _ => {}
            }
        }
        if let Some(status) = child.try_wait()? {
            // Drain what the readers still hold, without blocking on grandchildren that
            // inherited the pipes.
            let deadline = Instant::now() + Duration::from_millis(500);
            while let Ok((stream, line)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                if let Some(l) = log {
                    l.line(&line);
                }
                on_line(stream, &line);
                lines.push(line);
            }
            break status;
        }
    };
    drop(readers);

    if cancelled_at.is_some() {
        kill_group(pgid, libc::SIGKILL);
        if let Some(l) = log {
            l.line("cancelled");
        }
        return Err(ProcError::Cancelled);
    }
    if let Some(l) = log {
        l.line(&format!("exit: {}", describe(status)));
    }
    Ok(Output { status, lines })
}

/// Like [`run`] but a non-zero exit is an error.
pub fn run_checked(
    cmd: &Cmd,
    log: Option<&OpLog>,
    on_line: &mut dyn FnMut(Stream, &str),
    cancel: &Cancel,
) -> Result<Output, ProcError> {
    let out = run(cmd, log, on_line, cancel)?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(ProcError::Failed { program: program_name(cmd), status: describe(out.status), code: out.status.code() })
    }
}

/// Run quietly and capture stdout as a string (for short queries like `ldconfig -p`).
pub fn capture(cmd: &Cmd) -> Result<(ExitStatus, String), ProcError> {
    let out = cmd
        .command()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|source| ProcError::Spawn { program: program_name(cmd), source })?;
    Ok((out.status, String::from_utf8_lossy(&out.stdout).into_owned()))
}

pub fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(c), _) => format!("exit code {c}"),
        (None, Some(s)) => format!("signal {s}"),
        _ => "unknown status".into(),
    }
}

fn kill_group(pgid: i32, sig: i32) {
    // SAFETY: plain syscall; a stale pgid only yields ESRCH.
    unsafe {
        libc::kill(-pgid, sig);
    }
}

fn spawn_reader(
    pipe: impl Read + Send + 'static,
    stream: Stream,
    tx: mpsc::Sender<(Stream, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf);
                    let line = line.trim_end_matches(['\n', '\r']);
                    if tx.send((stream, line.to_string())).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// Find an executable on `PATH`.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| is_executable(p))
}

pub fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Cmd {
        Cmd::new("sh").arg("-c").arg(script)
    }

    #[test]
    fn streams_lines_and_writes_log() {
        let tmp = tempfile::tempdir().unwrap();
        let log = OpLog::create(tmp.path(), "test-op").unwrap();
        let mut seen = Vec::new();
        let out = run(
            &sh("echo one; echo two >&2; printf 'three'; exit 0").env("FOO", "bar"),
            Some(&log),
            &mut |s, l| seen.push((s, l.to_string())),
            &Cancel::new(),
        )
        .unwrap();
        assert!(out.status.success());
        assert!(seen.contains(&(Stream::Stdout, "one".into())));
        assert!(seen.contains(&(Stream::Stderr, "two".into())));
        assert!(seen.contains(&(Stream::Stdout, "three".into())));

        let name = log.path().file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with("-test-op.log") && name.starts_with("20"), "{name}");
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("$ sh -c 'echo one;"), "{text}");
        assert!(text.contains(" one\n") && text.contains(" two\n") && text.contains("exit: exit code 0"));
    }

    #[test]
    fn env_is_passed() {
        let mut seen = String::new();
        run(&sh("echo $LOIS_T").env("LOIS_T", "hello"), None, &mut |_, l| seen = l.into(), &Cancel::new()).unwrap();
        assert_eq!(seen, "hello");
    }

    #[test]
    fn nonzero_exit_is_reported() {
        let err = run_checked(&sh("echo boom; exit 3"), None, &mut |_, _| {}, &Cancel::new()).unwrap_err();
        assert!(matches!(err, ProcError::Failed { code: Some(3), .. }), "{err:?}");
    }

    #[test]
    fn cancel_kills_the_process_group() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("child.pid");
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        let start = Instant::now();
        // The grandchild `sleep` must die too.
        let script = format!("sleep 30 & echo $! > {}; echo started; wait", pidfile.display());
        let err = run(&sh(&script), None, &mut |_, l| if l == "started" { c2.cancel() }, &cancel).unwrap_err();
        assert!(matches!(err, ProcError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(10));
        let pid: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        // Gone, or a zombie waiting to be reaped by init.
        let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| !s.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'))
            .unwrap_or(false);
        assert!(!alive, "grandchild survived cancel");
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let err = run(&Cmd::new("/nonexistent/prog"), None, &mut |_, _| {}, &Cancel::new()).unwrap_err();
        assert!(matches!(err, ProcError::Spawn { .. }));
    }
}
