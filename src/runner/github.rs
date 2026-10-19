//! Soda runner releases from `bottlesdevs/wine` on GitHub
//! (wine-runner-management: List available runners).

use std::time::{Duration, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::Deserialize;

use crate::core::time::utc_parts;

pub const API_URL: &str = "https://api.github.com/repos/bottlesdevs/wine/releases";
pub const DOWNLOAD_BASE: &str = "https://github.com/bottlesdevs/wine/releases/download";
pub const DEFAULT_RUNNER: &str = "soda-11.0-32-experimental";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
    /// `YYYY-MM-DD`
    pub date: String,
    /// Full timestamp, for sorting.
    pub published_at: String,
    pub archive_url: String,
    pub archive_name: String,
    pub archive_size: u64,
    /// `None` when the release has no SHA256SUMS; such a runner cannot be verified or installed.
    pub sums_url: Option<String>,
}

impl Release {
    /// The release assets by their conventional URLs, without asking the API (used for the
    /// default runner, so a first install does not depend on the API rate limit).
    pub fn by_convention(tag: &str) -> Self {
        let archive_name = format!("{tag}-x86_64.tar.xz");
        Self {
            tag: tag.into(),
            prerelease: true,
            date: String::new(),
            published_at: String::new(),
            archive_url: format!("{DOWNLOAD_BASE}/{tag}/{archive_name}"),
            archive_name,
            archive_size: 0,
            sums_url: Some(format!("{DOWNLOAD_BASE}/{tag}/SHA256SUMS")),
        }
    }
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    #[serde(default)]
    size: u64,
    browser_download_url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GithubError {
    #[error("GitHub could not be reached. Check your internet connection. ({0})")]
    Offline(String),
    #[error("GitHub's API limit for your network is used up. Try again after {0}.")]
    RateLimited(String),
    #[error("GitHub answered {0}")]
    Status(u16),
    #[error("unexpected answer from GitHub: {0}")]
    Parse(String),
}

/// Parse one page of the releases API: `soda-*` tags with an x86_64 archive, newest first.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, GithubError> {
    let api: Vec<ApiRelease> = parse_json(json)?;
    let mut out: Vec<Release> = api
        .into_iter()
        .filter(|r| !r.draft && r.tag_name.starts_with("soda-"))
        .filter_map(|r| {
            let archive = r.assets.iter().find(|a| {
                a.name.ends_with(".tar.xz") && a.name.contains("x86_64") && !a.name.contains("symbols")
            })?;
            let sums = r.assets.iter().find(|a| a.name == "SHA256SUMS");
            let published_at = r.published_at.clone().unwrap_or_default();
            Some(Release {
                date: published_at.get(..10).unwrap_or("").to_string(),
                published_at,
                tag: r.tag_name.clone(),
                prerelease: r.prerelease,
                archive_url: archive.browser_download_url.clone(),
                archive_name: archive.name.clone(),
                archive_size: archive.size,
                sums_url: sums.map(|s| s.browser_download_url.clone()),
            })
        })
        .collect();
    out.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    Ok(out)
}

fn parse_json<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, GithubError> {
    serde_json::from_str(json).map_err(|e| GithubError::Parse(e.to_string()))
}

fn reset_time(header: Option<&str>) -> String {
    header
        .and_then(|v| v.parse::<u64>().ok())
        .map(|secs| {
            let (_, _, _, h, m, _) = utc_parts(UNIX_EPOCH + Duration::from_secs(secs));
            format!("{h:02}:{m:02} UTC")
        })
        .unwrap_or_else(|| "a while".into())
}

/// Fetch every page of releases from `api_url`.
pub fn fetch_releases(client: &Client, api_url: &str) -> Result<Vec<Release>, GithubError> {
    let mut all = Vec::new();
    for page in 1..=5 {
        let url = format!("{api_url}?per_page=100&page={page}");
        let resp = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .map_err(|e| GithubError::Offline(e.to_string()))?;
        let status = resp.status().as_u16();
        let header = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        if status == 403 || status == 429 {
            if header("x-ratelimit-remaining").as_deref() == Some("0") || status == 429 {
                return Err(GithubError::RateLimited(reset_time(header("x-ratelimit-reset").as_deref())));
            }
            return Err(GithubError::Status(status));
        }
        if !(200..300).contains(&status) {
            return Err(GithubError::Status(status));
        }
        let text = resp.text().map_err(|e| GithubError::Offline(e.to_string()))?;
        let raw: Vec<serde_json::Value> = parse_json(&text)?;
        let count = raw.len();
        all.extend(parse_releases(&text)?);
        if count < 100 {
            break;
        }
    }
    all.sort_by(|a, b| b.published_at.cmp(&a.published_at));
    all.dedup_by(|a, b| a.tag == b.tag);
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::download::http_client;
    use crate::core::download::testserver::{Route, Server};

    const FIXTURE: &str = include_str!("../../tests/fixtures/github-releases.json");

    #[test]
    fn parses_recorded_api_json() {
        let list = parse_releases(FIXTURE).unwrap();
        assert!(list.iter().all(|r| r.tag.starts_with("soda-")), "caffe is filtered out");
        assert_eq!(list[0].tag, "soda-11.0-32-experimental");
        assert!(list[0].prerelease);
        assert_eq!(list[0].date, "2026-10-17");
        assert_eq!(list[0].archive_name, "soda-11.0-32-experimental-x86_64.tar.xz");
        assert!(list[0].sums_url.as_deref().unwrap().ends_with("/SHA256SUMS"));
        assert_eq!(list[1].tag, "soda-11.0-31-experimental");
        assert!(list.windows(2).all(|w| w[0].published_at >= w[1].published_at), "newest first");
        let old = list.iter().find(|r| r.tag == "soda-11.0-25-experimental").unwrap();
        assert!(old.sums_url.is_none());
        let conv = Release::by_convention(DEFAULT_RUNNER);
        assert_eq!(conv.archive_url, list[0].archive_url);
        assert_eq!(conv.sums_url, list[0].sums_url);
    }

    #[test]
    fn rate_limit_and_offline() {
        let srv = Server::start();
        let mut r = Route::ok("{\"message\":\"API rate limit exceeded\"}");
        r.status = 403;
        r.extra_headers = vec![
            ("x-ratelimit-remaining".into(), "0".into()),
            ("x-ratelimit-reset".into(), "1792409137".into()),
        ];
        srv.route("/releases?per_page=100&page=1", r);
        let url = format!("{}/releases", srv.base);
        match fetch_releases(&http_client(), &url) {
            Err(GithubError::RateLimited(t)) => assert!(t.ends_with("UTC"), "{t}"),
            other => panic!("{other:?}"),
        }
        let err = fetch_releases(&http_client(), "http://127.0.0.1:9/releases").unwrap_err();
        assert!(matches!(err, GithubError::Offline(_)));
    }

    #[test]
    fn fetches_from_a_server() {
        let srv = Server::start();
        // The test server matches the full request path, query string included.
        srv.route("/releases?per_page=100&page=1", Route::ok(FIXTURE));
        let url = format!("{}/releases", srv.base);
        let list = fetch_releases(&http_client(), &url).unwrap();
        assert_eq!(list[0].tag, "soda-11.0-32-experimental");
    }

    /// `cargo test -- --ignored live_github` lists real releases.
    #[test]
    #[ignore]
    fn live_github_lists_default() {
        let list = fetch_releases(&http_client(), API_URL).unwrap();
        assert!(list.iter().any(|r| r.tag == DEFAULT_RUNNER), "{:?}", list.iter().map(|r| &r.tag).collect::<Vec<_>>());
    }
}
