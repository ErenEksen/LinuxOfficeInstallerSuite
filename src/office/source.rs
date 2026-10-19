//! Local Office source (office-installation: Local Office source).
//!
//! Click-to-Run under Wine has no working BITS, so `setup.exe /configure` falls back to one
//! slow HTTP connection and needs hours for the ~3 GB payload. The payload is fetched here
//! instead, natively and over parallel `Range` connections, into
//! `~/.msoffice/cache/office-source/<channel>/` in the layout `setup.exe /download` produces.
//! Setup then installs from that folder (`SourcePath`, through the prefix's `Z:` drive).
//! The cache is kept, so a later install of the same build downloads nothing.

use std::path::{Path, PathBuf};

use reqwest::blocking::Client;

use super::config_xml::{DeploymentConfig, Edition};
use super::plans::Channel;
use crate::core::download::{self, DownloadError, Meter, RangedOpts, RemoteFile};
use crate::core::paths::Paths;
use crate::core::proc::{Cmd, OpLog, ProcError, run};
use crate::core::{Cancel, Report};

pub const CDN: &str = "https://officecdn.microsoft.com/pr";

/// Files at least this big use parallel range downloads.
const RANGED_MIN: u64 = 32 << 20;

/// CDN folder of each update channel.
pub fn channel_guid(c: Channel) -> &'static str {
    match c {
        Channel::Current => "492350f6-3a01-4f97-b9c0-c7c6ddf67d60",
        Channel::MonthlyEnterprise => "55336b82-a18d-4dd6-b5f6-9e5095c314a6",
        Channel::SemiAnnual => "7ffbc6bf-bc32-4f92-8982-f9dd17fd3114",
    }
}

/// `("64", "x64")` or `("32", "x86")`: the cab and stream name parts of an edition.
fn arch(e: Edition) -> (&'static str, &'static str) {
    match e {
        Edition::X64 => ("64", "x64"),
        Edition::X86 => ("32", "x86"),
    }
}

/// Office culture tags and their LCIDs (used in the `i64<lcid>.cab` and `s64<lcid>.cab` names).
const CULTURES: &[(&str, u32)] = &[
    ("ar-sa", 1025),
    ("bg-bg", 1026),
    ("ca-es", 1027),
    ("cs-cz", 1029),
    ("da-dk", 1030),
    ("de-de", 1031),
    ("el-gr", 1032),
    ("en-gb", 2057),
    ("en-us", 1033),
    ("es-es", 3082),
    ("es-mx", 2058),
    ("et-ee", 1061),
    ("eu-es", 1069),
    ("fi-fi", 1035),
    ("fr-ca", 3084),
    ("fr-fr", 1036),
    ("gl-es", 1110),
    ("he-il", 1037),
    ("hi-in", 1081),
    ("hr-hr", 1050),
    ("hu-hu", 1038),
    ("id-id", 1057),
    ("it-it", 1040),
    ("ja-jp", 1041),
    ("kk-kz", 1087),
    ("ko-kr", 1042),
    ("lt-lt", 1063),
    ("lv-lv", 1062),
    ("ms-my", 1086),
    ("nb-no", 1044),
    ("nl-nl", 1043),
    ("pl-pl", 1045),
    ("pt-br", 1046),
    ("pt-pt", 2070),
    ("ro-ro", 1048),
    ("ru-ru", 1049),
    ("sk-sk", 1051),
    ("sl-si", 1060),
    ("sr-latn-rs", 9242),
    ("sv-se", 1053),
    ("th-th", 1054),
    ("tr-tr", 1055),
    ("uk-ua", 1058),
    ("vi-vn", 1066),
    ("zh-cn", 2052),
    ("zh-tw", 1028),
];

pub fn lcid(culture: &str) -> Option<u32> {
    CULTURES.iter().find(|(c, _)| *c == culture).map(|(_, l)| *l)
}

/// `tr_TR.UTF-8` → `tr-tr`, `sr_RS@latin` → `sr-latn-rs`; `C`/`POSIX` → none.
pub fn culture_from_locale(locale: &str) -> Option<String> {
    let (base, modifier) = match locale.split_once('@') {
        Some((b, m)) => (b, Some(m)),
        None => (locale, None),
    };
    let base = base.split('.').next()?;
    let (lang, region) = base.split_once('_')?;
    let (lang, region) = (lang.to_ascii_lowercase(), region.to_ascii_lowercase());
    if lang.is_empty() || region.is_empty() {
        return None;
    }
    Some(match modifier {
        Some("latin") => format!("{lang}-latn-{region}"),
        _ => format!("{lang}-{region}"),
    })
}

/// The UI locale Wine (and so `MatchOS`) sees: `LC_ALL`, then `LC_MESSAGES`, then `LANG`.
pub fn os_culture(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| env(k).filter(|v| !v.is_empty()))
        .next()
        .and_then(|l| culture_from_locale(&l))
}

/// Cultures whose language files are fetched. `MatchOS` resolves to the OS culture plus
/// `en-us`. Unknown cultures are left to setup's CDN fallback.
pub fn cultures(cfg: &DeploymentConfig, os: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let wanted: Vec<String> = if cfg.match_os {
        os.into_iter().map(str::to_string).chain(["en-us".to_string()]).collect()
    } else {
        cfg.languages.iter().map(|l| l.trim().to_ascii_lowercase()).collect()
    };
    for c in wanted {
        if lcid(&c).is_some() && !out.contains(&c) {
            out.push(c);
        }
    }
    if out.is_empty() {
        out.push("en-us".into());
    }
    out
}

/// The build a channel currently offers, from its `VersionDescriptor.xml`.
pub fn parse_version(xml: &str) -> Option<String> {
    let re = regex::Regex::new(r#"<AvailableBuild\s+RestrictionType="Default"\s+Version="(16\.[0-9.]+)""#).unwrap();
    re.captures(xml).map(|c| c[1].to_string())
}

/// A payload file, as a path under `Office/Data/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub rel: String,
    /// Optional files may be missing on the CDN (small per-language cabs).
    pub required: bool,
}

/// Everything `setup.exe /download` would fetch for this edition, build and cultures.
pub fn file_list(edition: Edition, version: &str, cultures: &[String]) -> Vec<SourceFile> {
    let (a, p) = arch(edition);
    let req = |rel: String| SourceFile { rel, required: true };
    let mut files = vec![
        req(format!("v{a}_{version}.cab")),
        req(format!("{version}/i{a}0.cab")),
        req(format!("{version}/s{a}0.cab")),
        req(format!("{version}/stream.{p}.x-none.dat")),
    ];
    for c in cultures {
        let Some(l) = lcid(c) else { continue };
        files.push(SourceFile { rel: format!("{version}/i{a}{l}.cab"), required: false });
        files.push(req(format!("{version}/s{a}{l}.cab")));
        files.push(req(format!("{version}/stream.{p}.{c}.dat")));
    }
    files
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// The CDN does not look as expected; setup can still download by itself.
    #[error("{0}")]
    Layout(String),
    #[error("{0}")]
    Download(DownloadError),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl From<DownloadError> for SourceError {
    fn from(e: DownloadError) -> Self {
        match e {
            DownloadError::Cancelled => SourceError::Cancelled,
            other => SourceError::Download(other),
        }
    }
}

/// A finished local source: the folder that holds `Office/`, and the build in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSource {
    pub dir: PathBuf,
    pub version: String,
}

impl LocalSource {
    /// The folder as setup sees it through the `Z:` drive.
    pub fn windows_path(&self) -> String {
        format!("Z:{}", self.dir.display()).replace('/', "\\")
    }
}

pub fn cache_dir(paths: &Paths, channel: Channel) -> PathBuf {
    paths.cache().join("office-source").join(channel.odt_name())
}

/// Make sure the prefix maps `Z:` to `/`, which `SourcePath` relies on.
pub fn ensure_z_drive(prefix: &Path) -> std::io::Result<()> {
    let link = prefix.join("dosdevices/z:");
    if link.symlink_metadata().is_err() {
        std::fs::create_dir_all(prefix.join("dosdevices"))?;
        std::os::unix::fs::symlink("/", &link)?;
    }
    Ok(())
}

/// Download (or reuse) the payload for `cfg`. `report` gets progress, a live detail line and
/// log lines.
pub fn prepare(
    client: &Client,
    paths: &Paths,
    cfg: &DeploymentConfig,
    log: &OpLog,
    report: &mut dyn FnMut(Report),
    cancel: &Cancel,
) -> Result<LocalSource, SourceError> {
    let channel = cfg.plan.effective_channel(cfg.channel);
    let base = format!("{CDN}/{}/Office/Data", channel_guid(channel));
    let dir = cache_dir(paths, channel);
    let data = dir.join("Office/Data");
    std::fs::create_dir_all(&data)?;
    let (a, _) = arch(cfg.edition);

    report(Report::Detail("Checking the current Office build".into()));
    let version = resolve_version(client, &base, a, &dir, log, cancel)?;
    let os = os_culture(|k| std::env::var(k).ok());
    let cultures = cultures(cfg, os.as_deref());
    log.line(&format!("Office build {version} on {}, languages {cultures:?} (OS: {os:?})", channel.odt_name()));
    prune_other_versions(&data, &version, log);

    // Sizes first, so progress covers the whole payload.
    let mut plan: Vec<(String, RemoteFile)> = Vec::new();
    for f in file_list(cfg.edition, &version, &cultures) {
        if cancel.is_cancelled() {
            return Err(SourceError::Cancelled);
        }
        let url = format!("{base}/{}", f.rel);
        match download::head(client, &url) {
            Ok(Some(r)) => plan.push((f.rel, r)),
            Ok(None) if !f.required => log.line(&format!("not on the CDN, skipped: {}", f.rel)),
            Ok(None) | Err(DownloadError::Status { .. }) => {
                return Err(SourceError::Layout(format!("{url} is not available on the Office CDN")));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let total: u64 = plan.iter().map(|(_, r)| r.size).sum();
    let cached: u64 = plan
        .iter()
        .filter(|(rel, r)| data.join(rel).metadata().is_ok_and(|m| m.len() == r.size))
        .map(|(_, r)| r.size)
        .sum();
    log.line(&format!(
        "Office files: {} in {} files, {} already cached in {}",
        download::format_bytes(total),
        plan.len(),
        download::format_bytes(cached),
        dir.display()
    ));

    let mut meter = Meter::new();
    let mut finished = 0u64;
    for (rel, remote) in &plan {
        let dest = data.join(rel);
        let url = format!("{base}/{rel}");
        if dest.metadata().is_ok_and(|m| m.len() != remote.size) {
            std::fs::remove_file(&dest)?;
        }
        let mut on_progress = |d: u64, _: Option<u64>| {
            let now = finished + d.min(remote.size);
            if total > 0 {
                report(Report::Progress(Some(now as f32 / total as f32)));
            }
            let u = meter.update(now, Some(total));
            if let Some(live) = u.live {
                report(Report::Detail(format!("Downloading Office: {live}")));
            }
            if let Some(line) = u.log {
                let line = format!("Office download: {line}");
                log.line(&line);
                report(Report::Line(line));
            }
        };
        if remote.size >= RANGED_MIN {
            download::download_ranged(client, &url, &dest, remote, RangedOpts::default(), &mut on_progress, cancel)?;
        } else {
            download::download(client, &url, &dest, &[], &mut on_progress, cancel)?;
            let got = dest.metadata()?.len();
            if got != remote.size {
                let _ = std::fs::remove_file(&dest);
                return Err(SourceError::Download(DownloadError::Io(std::io::Error::other(format!(
                    "{url}: got {got} bytes, expected {}",
                    remote.size
                )))));
            }
        }
        finished += remote.size;
    }
    // `setup.exe /download` leaves the build's cab also as `v64.cab`.
    std::fs::copy(data.join(format!("v{a}_{version}.cab")), data.join(format!("v{a}.cab")))?;
    Ok(LocalSource { dir, version })
}

fn resolve_version(
    client: &Client,
    base: &str,
    a: &str,
    dir: &Path,
    log: &OpLog,
    cancel: &Cancel,
) -> Result<String, SourceError> {
    let work = dir.join(".version-check");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let cab = work.join(format!("v{a}.cab"));
    download::download(client, &format!("{base}/v{a}.cab"), &cab, &[], &mut |_, _| {}, cancel).map_err(|e| match e {
        DownloadError::Status { url, status } => SourceError::Layout(format!("{url}: server answered {status}")),
        other => other.into(),
    })?;
    let cmd = Cmd::new("cabextract").arg("-q").arg("-d").arg(&work).arg("-F").arg("VersionDescriptor.xml").arg(&cab);
    match run(&cmd, Some(log), &mut |_, _| {}, cancel) {
        Ok(out) if out.status.success() => {}
        Ok(out) => return Err(SourceError::Layout(format!("could not read v{a}.cab:\n{}", out.tail(5)))),
        Err(ProcError::Cancelled) => return Err(SourceError::Cancelled),
        Err(e) => return Err(SourceError::Layout(format!("could not read v{a}.cab: {e}"))),
    }
    let xml = std::fs::read(work.join("VersionDescriptor.xml"))?;
    let xml = String::from_utf8_lossy(&xml);
    let version = parse_version(&xml).ok_or_else(|| SourceError::Layout(format!("no build listed in v{a}.cab")))?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(version)
}

/// Delete cached builds other than `version`, so the cache holds one build per channel.
fn prune_other_versions(data: &Path, version: &str, log: &OpLog) {
    let Ok(entries) = std::fs::read_dir(data) else { return };
    let keep_cab = |name: &str| {
        // v64.cab / v32.cab are rewritten after every download; v64_<build>.cab must match.
        name == "v64.cab" || name == "v32.cab" || name.strip_suffix(".cab").and_then(|n| n.split_once('_')).is_some_and(|(_, v)| v == version)
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let stale = if path.is_dir() { name.starts_with("16.") && name != version } else { name.ends_with(".cab") && !keep_cab(&name) };
        if stale {
            log.line(&format!("removing the cached old Office build file {}", path.display()));
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::office::plans::Plan;

    #[test]
    fn version_from_descriptor() {
        let xml = r#"<Version><Available Build="16.0.12527.22286" I640Version="16.0.20430.20182" />
<AvailableBuilds>
<AvailableBuild RestrictionType="Default" Version="16.0.20430.20182" />
<AvailableBuild RestrictionType="OS" Value="7" Version="16.0.12527.22286" />
</AvailableBuilds></Version>"#;
        assert_eq!(parse_version(xml).as_deref(), Some("16.0.20430.20182"));
        assert_eq!(parse_version("<Version />"), None);
    }

    #[test]
    fn locales_map_to_office_cultures() {
        assert_eq!(culture_from_locale("tr_TR.UTF-8").as_deref(), Some("tr-tr"));
        assert_eq!(culture_from_locale("en_US").as_deref(), Some("en-us"));
        assert_eq!(culture_from_locale("sr_RS@latin").as_deref(), Some("sr-latn-rs"));
        assert_eq!(culture_from_locale("C.UTF-8"), None);
        assert_eq!(culture_from_locale("POSIX"), None);
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(os_culture(env(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "tr_TR.UTF-8")])).as_deref(), Some("en-us"));
        assert_eq!(os_culture(env(&[("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "de_DE.UTF-8")])).as_deref(), Some("de-de"));
        assert_eq!(os_culture(env(&[("LC_ALL", ""), ("LANG", "fr_FR.UTF-8")])).as_deref(), Some("fr-fr"));
    }

    #[test]
    fn match_os_adds_en_us_and_skips_unknown_cultures() {
        let cfg = DeploymentConfig::default();
        assert_eq!(cultures(&cfg, Some("tr-tr")), ["tr-tr", "en-us"]);
        assert_eq!(cultures(&cfg, Some("en-us")), ["en-us"]);
        assert_eq!(cultures(&cfg, Some("de-at")), ["en-us"]);
        assert_eq!(cultures(&cfg, None), ["en-us"]);
        let cfg = DeploymentConfig { match_os: false, languages: vec!["DE-de".into(), "xx-yy".into()], ..Default::default() };
        assert_eq!(cultures(&cfg, Some("tr-tr")), ["de-de"]);
    }

    #[test]
    fn file_list_matches_the_odt_download_layout() {
        let v = "16.0.20430.20182";
        let names: Vec<String> = file_list(Edition::X64, v, &["en-us".into()]).into_iter().map(|f| f.rel).collect();
        assert_eq!(
            names,
            [
                "v64_16.0.20430.20182.cab",
                "16.0.20430.20182/i640.cab",
                "16.0.20430.20182/s640.cab",
                "16.0.20430.20182/stream.x64.x-none.dat",
                "16.0.20430.20182/i641033.cab",
                "16.0.20430.20182/s641033.cab",
                "16.0.20430.20182/stream.x64.en-us.dat",
            ]
        );
        let x86 = file_list(Edition::X86, v, &["tr-tr".into()]);
        assert!(x86.iter().any(|f| f.rel == "16.0.20430.20182/stream.x86.tr-tr.dat"));
        assert!(x86.iter().any(|f| f.rel == "16.0.20430.20182/s321055.cab"));
        assert!(!x86.iter().find(|f| f.rel.ends_with("i321055.cab")).unwrap().required);
    }

    #[test]
    fn source_path_goes_through_the_z_drive() {
        let s = LocalSource { dir: "/home/u/.msoffice/cache/office-source/Current".into(), version: "16.0.1.2".into() };
        assert_eq!(s.windows_path(), r"Z:\home\u\.msoffice\cache\office-source\Current");
        let tmp = tempfile::tempdir().unwrap();
        ensure_z_drive(tmp.path()).unwrap();
        assert_eq!(std::fs::read_link(tmp.path().join("dosdevices/z:")).unwrap(), Path::new("/"));
        ensure_z_drive(tmp.path()).unwrap();
    }

    #[test]
    fn retail_2024_uses_the_current_channel_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::for_home(tmp.path());
        let cfg = DeploymentConfig { plan: Plan::Home2024, channel: Channel::SemiAnnual, ..Default::default() };
        let ch = cfg.plan.effective_channel(cfg.channel);
        assert_eq!(channel_guid(ch), "492350f6-3a01-4f97-b9c0-c7c6ddf67d60");
        assert!(cache_dir(&paths, ch).ends_with(".msoffice/cache/office-source/Current"));
    }

    #[test]
    fn pruning_keeps_only_the_current_build() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path();
        for d in ["16.0.1.1", "16.0.2.2"] {
            std::fs::create_dir_all(data.join(d)).unwrap();
            std::fs::write(data.join(d).join("s640.cab"), b"x").unwrap();
        }
        for f in ["v64_16.0.1.1.cab", "v64_16.0.2.2.cab", "v32_16.0.1.1.cab", "v64.cab"] {
            std::fs::write(data.join(f), b"x").unwrap();
        }
        let log = OpLog::create(&tmp.path().join("logs"), "t").unwrap();
        prune_other_versions(data, "16.0.2.2", &log);
        let mut left: Vec<String> =
            std::fs::read_dir(data).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["16.0.2.2", "logs", "v64.cab", "v64_16.0.2.2.cab"]);
    }

    /// Fill the real cache for the default configuration:
    /// `cargo test -- --ignored live_prepare --nocapture`. With `LOIS_SOURCE_SECONDS=<n>` it
    /// cancels after n seconds (the chunks stay and the next run resumes).
    #[test]
    #[ignore]
    fn live_prepare() {
        let paths = Paths::from_env();
        let log = OpLog::create(&paths.logs(), "source").unwrap();
        let cancel = Cancel::new();
        if let Some(secs) = std::env::var("LOIS_SOURCE_SECONDS").ok().and_then(|v| v.parse().ok()) {
            let c = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                c.cancel();
            });
        }
        let mut last = String::new();
        let res = prepare(&download::http_client(), &paths, &DeploymentConfig::default(), &log, &mut |r| match r {
            Report::Detail(d) => last = d,
            Report::Line(l) => eprintln!("{l}"),
            Report::Progress(_) => {}
        }, &cancel);
        eprintln!("last status: {last}\nresult: {res:?}");
        if std::env::var_os("LOIS_SOURCE_SECONDS").is_none() {
            let s = res.unwrap();
            assert!(s.dir.join(format!("Office/Data/{}/stream.x64.x-none.dat", s.version)).exists());
            assert!(s.dir.join("Office/Data/v64.cab").exists());
        }
    }

    /// Live check against the Office CDN, without the big download:
    /// `cargo test -- --ignored live_cdn_layout --nocapture`.
    #[test]
    #[ignore]
    fn live_cdn_layout() {
        let client = download::http_client();
        for ch in Channel::ALL {
            let base = format!("{CDN}/{}/Office/Data", channel_guid(ch));
            let tmp = tempfile::tempdir().unwrap();
            let log = OpLog::create(&tmp.path().join("logs"), "t").unwrap();
            for e in [Edition::X64, Edition::X86] {
                let v = resolve_version(&client, &base, arch(e).0, tmp.path(), &log, &Cancel::new()).unwrap();
                let mut total = 0;
                for f in file_list(e, &v, &["en-us".into(), "tr-tr".into()]) {
                    let r = download::head(&client, &format!("{base}/{}", f.rel)).unwrap();
                    assert!(r.is_some() || !f.required, "{ch:?} {e:?} missing {}", f.rel);
                    total += r.map(|r| r.size).unwrap_or(0);
                }
                eprintln!("{ch:?} {e:?} {v}: {}", download::format_bytes(total));
            }
        }
    }
}
