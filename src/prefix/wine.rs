//! Running Wine against one prefix: environment, registry edits, DLL overrides, Windows
//! version and wineserver control (design D5).

use std::path::PathBuf;

use crate::core::Cancel;
use crate::core::proc::{Cmd, OpLog, Output, ProcError, Stream, run, run_checked};

/// Menu builder off for every process we start, so Wine never writes its own launchers
/// or file associations (wine-prefix-setup: Prefix location and architecture).
pub const BASE_OVERRIDES: &str = "winemenubuilder.exe=d";

#[derive(Clone, Debug)]
pub struct Wine {
    /// Runner root, containing `bin/wine`.
    pub runner: PathBuf,
    pub prefix: PathBuf,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegType {
    Sz,
    Dword,
}

#[cfg(test)]
impl RegType {
    fn flag(self) -> &'static str {
        match self {
            RegType::Sz => "REG_SZ",
            RegType::Dword => "REG_DWORD",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WinVersion {
    Win7,
    Win10,
}

impl Wine {
    pub fn new(runner: impl Into<PathBuf>, prefix: impl Into<PathBuf>) -> Self {
        Self { runner: runner.into(), prefix: prefix.into() }
    }

    pub fn bin(&self, name: &str) -> PathBuf {
        self.runner.join("bin").join(name)
    }

    /// A command with the prefix environment set. `program` is a file in the runner's `bin/`.
    pub fn tool(&self, program: &str) -> Cmd {
        Cmd::new(self.bin(program))
            .env("WINEPREFIX", &self.prefix)
            .env("WINEARCH", "win64")
            .env("WINEDLLOVERRIDES", BASE_OVERRIDES)
            .env("WINEDEBUG", "fixme-all")
            // Never let a caller's Wine settings leak into our prefix.
            .env_remove("WINELOADER")
            .env_remove("WINESERVER")
            .env_remove("WINEDLLPATH")
    }

    /// `wine <args…>`
    pub fn wine<I: IntoIterator<Item = S>, S: Into<std::ffi::OsString>>(&self, args: I) -> Cmd {
        self.tool("wine").args(args)
    }

    #[cfg(test)]
    pub fn reg_add(&self, key: &str, value: Option<&str>, ty: RegType, data: &str) -> Cmd {
        let mut c = self.wine(["reg", "add", key]);
        c = match value {
            Some(v) => c.args(["/v", v]),
            None => c.arg("/ve"),
        };
        c.args(["/t", ty.flag(), "/d", data, "/f"])
    }

    pub fn reg_delete(&self, key: &str, value: Option<&str>) -> Cmd {
        let c = self.wine(["reg", "delete", key]);
        match value {
            Some(v) => c.args(["/v", v, "/f"]),
            None => c.arg("/f"),
        }
    }

    #[cfg(test)]
    pub fn reg_query(&self, key: &str, value: &str) -> Cmd {
        self.wine(["reg", "query", key, "/v", value])
    }

    #[cfg(test)]
    pub fn set_override(&self, dll: &str, mode: &str) -> Cmd {
        self.reg_add(r"HKCU\Software\Wine\DllOverrides", Some(dll), RegType::Sz, mode)
    }

    /// Apply many values with one `wine reg import`. Keys use full hive names
    /// (`HKEY_LOCAL_MACHINE\\…`).
    pub fn reg_import(
        &self,
        entries: &[(String, String, RegData)],
        log: Option<&OpLog>,
        cancel: &Cancel,
    ) -> Result<(), ProcError> {
        let temp = self.drive_c().join("windows/temp");
        std::fs::create_dir_all(&temp)?;
        let name = format!("lois-import-{}.reg", std::process::id());
        let file = temp.join(&name);
        std::fs::write(&file, render_reg_file(entries))?;
        let result = run_checked(
            &self.wine(["reg", "import", &format!("C:\\windows\\temp\\{name}")]),
            log,
            &mut |_, _| {},
            cancel,
        );
        let _ = std::fs::remove_file(&file);
        result.map(|_| ())
    }

    pub fn wineserver_wait(&self) -> Cmd {
        self.tool("wineserver").arg("-w")
    }

    pub fn wineserver_kill(&self) -> Cmd {
        self.tool("wineserver").arg("-k")
    }

    /// Run a command and fail on non-zero exit.
    pub fn exec(
        &self,
        cmd: &Cmd,
        log: Option<&OpLog>,
        on_line: &mut dyn FnMut(Stream, &str),
        cancel: &Cancel,
    ) -> Result<Output, ProcError> {
        run_checked(cmd, log, on_line, cancel)
    }

    /// Read one registry value (`None` if the key or value does not exist).
    #[cfg(test)]
    pub fn query_value(&self, key: &str, value: &str) -> Result<Option<String>, ProcError> {
        let out = run(&self.reg_query(key, value), None, &mut |_, _| {}, &Cancel::new())?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(parse_reg_query(&out.lines.join("\n"), value))
    }

    /// Stop every process of this prefix; waits for the server to exit.
    pub fn kill_all(&self) {
        let _ = run(&self.wineserver_kill(), None, &mut |_, _| {}, &Cancel::new());
    }

    pub fn drive_c(&self) -> PathBuf {
        self.prefix.join("drive_c")
    }

    pub fn system32(&self) -> PathBuf {
        self.drive_c().join("windows/system32")
    }

    pub fn syswow64(&self) -> PathBuf {
        self.drive_c().join("windows/syswow64")
    }

    pub fn fonts_dir(&self) -> PathBuf {
        self.drive_c().join("windows/Fonts")
    }

    pub fn is_initialised(&self) -> bool {
        self.prefix.join("system.reg").exists() && self.system32().is_dir()
    }
}

/// Registry values for a Windows version: the `CurrentVersion` keys winecfg writes, plus
/// the global `HKCU\Software\Wine` `Version` value, which takes precedence in Wine.
pub fn windows_version_entries(v: WinVersion) -> Vec<(String, String, RegData)> {
    let nt = r"HKEY_LOCAL_MACHINE\Software\Microsoft\Windows NT\CurrentVersion";
    let (name, ver, build, major, minor, csd, csd_num, wine) = match v {
        WinVersion::Win7 => ("Windows 7", "6.1", "7601", 6, 1, "Service Pack 1", 0x100, "win7"),
        WinVersion::Win10 => ("Windows 10 Pro", "6.3", "19045", 10, 0, "", 0, "win10"),
    };
    let sz = |k: &str, n: &str, d: &str| (k.to_string(), n.to_string(), RegData::Sz(d.to_string()));
    let dw = |k: &str, n: &str, d: u32| (k.to_string(), n.to_string(), RegData::Dword(d));
    let mut e = vec![
        sz(nt, "ProductName", name),
        sz(nt, "CurrentVersion", ver),
        sz(nt, "CurrentBuild", build),
        sz(nt, "CurrentBuildNumber", build),
        sz(nt, "CSDVersion", csd),
        dw(nt, "CurrentMajorVersionNumber", major),
        dw(nt, "CurrentMinorVersionNumber", minor),
    ];
    if v == WinVersion::Win10 {
        e.push(sz(nt, "DisplayVersion", "22H2"));
    }
    e.push(dw(r"HKEY_LOCAL_MACHINE\System\CurrentControlSet\Control\Windows", "CSDVersion", csd_num));
    e.push(sz(r"HKEY_LOCAL_MACHINE\System\CurrentControlSet\Control\ProductOptions", "ProductType", "WinNT"));
    e.push(sz(r"HKEY_CURRENT_USER\Software\Wine", "Version", wine));
    e
}

/// One value for [`Wine::reg_import`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegData {
    Sz(String),
    Dword(u32),
}

/// Render a `.reg` file (UTF-16LE with BOM, as `reg import` expects for version 5.00).
pub fn render_reg_file(entries: &[(String, String, RegData)]) -> Vec<u8> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
    let mut current_key: Option<&str> = None;
    for (key, name, data) in entries {
        if current_key != Some(key.as_str()) {
            text.push_str(&format!("\r\n[{key}]\r\n"));
            current_key = Some(key);
        }
        let value = match data {
            RegData::Sz(s) => format!("\"{}\"", esc(s)),
            RegData::Dword(d) => format!("dword:{d:08x}"),
        };
        text.push_str(&format!("\"{}\"={value}\r\n", esc(name)));
    }
    text.push_str("\r\n");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
    bytes
}

/// Extract the data of `value` from `reg query` output:
/// `    CurrentVersion    REG_SZ    6.3`
#[cfg(test)]
pub fn parse_reg_query(text: &str, value: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix(value) else { continue };
        let mut parts = rest.split_whitespace();
        let Some(ty) = parts.next() else { continue };
        if !ty.starts_with("REG_") {
            continue;
        }
        // Data may contain spaces: take everything after the type column.
        let idx = rest.find(ty).unwrap() + ty.len();
        return Some(rest[idx..].trim().to_string());
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reg_query_output() {
        let text = "HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\n    \
                    ProductName    REG_SZ    Windows 10 Pro\n    CurrentVersion    REG_SZ    6.3\n";
        assert_eq!(parse_reg_query(text, "ProductName").as_deref(), Some("Windows 10 Pro"));
        assert_eq!(parse_reg_query(text, "CurrentVersion").as_deref(), Some("6.3"));
        assert_eq!(parse_reg_query(text, "Missing"), None);
        let dword = "HKEY_CURRENT_USER\\Software\\Wine\\Direct3D\n    MaxVersionGL    REG_DWORD    0x30002\n";
        assert_eq!(parse_reg_query(dword, "MaxVersionGL").as_deref(), Some("0x30002"));
    }

    #[test]
    fn builds_reg_commands() {
        let w = Wine::new("/r", "/p");
        let c = w.set_override("gdiplus", "native");
        assert_eq!(
            c.display(),
            r"/r/bin/wine reg add 'HKCU\Software\Wine\DllOverrides' /v gdiplus /t REG_SZ /d native /f"
        );
        assert!(c.env.iter().any(|(k, v)| k == "WINEPREFIX" && v == "/p"));
        assert!(c.env.iter().any(|(k, v)| k == "WINEDLLOVERRIDES" && v == BASE_OVERRIDES));
        let d = w.reg_delete(r"HKCU\Software\Wine\Direct2D", Some("max_version_factory"));
        assert!(d.display().ends_with("/v max_version_factory /f"));
    }

    /// `cargo test -- --ignored live_wine_helpers` (uses the active runner of the test system).
    #[test]
    #[ignore]
    fn live_wine_helpers_read_back() {
        let paths = crate::core::paths::Paths::from_env();
        let tmp = tempfile::tempdir().unwrap();
        let w = Wine::new(paths.active_runner_link(), tmp.path().join("pfx"));
        let c = Cancel::new();
        let quiet = &mut |_: Stream, _: &str| {};
        w.exec(&w.wine(["wineboot", "-u"]), None, quiet, &c).unwrap();
        w.exec(&w.set_override("*gdiplus", "native"), None, quiet, &c).unwrap();
        w.reg_import(&windows_version_entries(WinVersion::Win10), None, &c).unwrap();
        w.exec(&w.reg_add(r"HKCU\Software\Wine\Direct3D", Some("MaxVersionGL"), RegType::Dword, "0x30002"), None, quiet, &c)
            .unwrap();
        w.exec(&w.reg_add(r"HKCU\Software\Wine\Direct2D", Some("max_version_factory"), RegType::Dword, "0x1"), None, quiet, &c)
            .unwrap();
        w.exec(&w.reg_delete(r"HKCU\Software\Wine\Direct2D", Some("max_version_factory")), None, quiet, &c).unwrap();
        run(&w.wineserver_wait(), None, quiet, &c).unwrap();

        assert_eq!(w.query_value(r"HKCU\Software\Wine\DllOverrides", "*gdiplus").unwrap().as_deref(), Some("native"));
        let nt = r"HKLM\Software\Microsoft\Windows NT\CurrentVersion";
        assert_eq!(w.query_value(nt, "CurrentBuild").unwrap().as_deref(), Some("19045"));
        assert_eq!(w.query_value(nt, "ProductName").unwrap().as_deref(), Some("Windows 10 Pro"));
        assert_eq!(w.query_value(r"HKCU\Software\Wine\Direct3D", "MaxVersionGL").unwrap().as_deref(), Some("0x30002"));
        assert_eq!(w.query_value(r"HKCU\Software\Wine\Direct2D", "max_version_factory").unwrap(), None);
        // What a Windows program sees: `ver` inside cmd.
        let out = run(&w.wine(["cmd", "/c", "ver"]), None, quiet, &c).unwrap();
        eprintln!("ver: {:?}", out.lines);
        assert!(out.lines.iter().any(|l| l.contains("10.0.19045")), "{:?}", out.lines);
        w.kill_all();
    }
}
