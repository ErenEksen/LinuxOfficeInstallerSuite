//! Microsoft account sign-in for the bridge: OAuth 2.0 authorization code with PKCE in the
//! system browser, then refresh tokens from the keyring.
//!
//! Microsoft sends the browser back to `ms-appx-web://microsoft.aad.brokerplugin/<client id>`,
//! the redirect URI the Windows broker uses. Our URL handler for that scheme
//! (`msoffice-identity callback`) passes the URI to the waiting sign-in over a socket named
//! after the request's `state`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE_NO_PAD, URL_SAFE_NO_PAD_INDIFFERENT};
use regex::Regex;
use sha2::{Digest, Sha256};

use super::store::{Kv, Record, Store, StoreError};

pub const PERSONAL_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
pub const CALLBACK_SCHEME: &str = "ms-appx-web";
const CALLBACK_HOST: &str = "microsoft.aad.brokerplugin";
const MAX_CALLBACK: usize = 64 * 1024;
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

/// Office asks by caller id for some requests; this maps it to the OAuth client id.
const CALLER_CLIENT_IDS: &[(&str, &str)] =
    &[("2b379600-b42b-4fe9-a59c-a312fb934935", "d3590ed6-52b3-4102-aeff-aad2292ab01c")];

/// Legacy (MSA ticket) scope names Office requests, and their v2 equivalents.
const SCOPE_ALIASES: &[(&str, &str)] = &[(
    "service::https://licensing.m365.svc.cloud.microsoft/::MBI_SSL_SHORT",
    "https://licensing.m365.svc.cloud.microsoft/.default",
)];

/// Resources Office asks for that a public client cannot get a token for.
const UNSUPPORTED_RESOURCES: &[&str] = &["https://events.data.microsoft.com/OneCollector/1.0/"];

static CLIENT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9.-]{1,128}$").unwrap());
static TENANT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(common|organizations|consumers|[0-9a-fA-F-]{36})$").unwrap());
static STATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{32,128}$").unwrap());
static GUID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$").unwrap());

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The request itself is malformed.
    #[error("{0}")]
    Invalid(String),
    #[error("authentication canceled")]
    Canceled,
    #[error("{0}")]
    Storage(String),
    #[error("{0}")]
    Failed(String),
}

impl From<StoreError> for AuthError {
    fn from(e: StoreError) -> Self {
        AuthError::Storage(e.0)
    }
}

fn invalid(what: &str) -> AuthError {
    AuthError::Invalid(format!("invalid {what}"))
}

fn failed(what: impl Into<String>) -> AuthError {
    AuthError::Failed(what.into())
}

/// A token for the caller, plus the account it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthResult {
    pub access_token: String,
    pub expires_at: u64,
    pub scopes: String,
    pub account: Record,
}

/// HTTPS form POST, returning the status and body.
pub trait Http: Send + Sync {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String>;
}

/// Opens a URL in the user's browser.
pub trait Browser: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

pub struct ReqwestHttp(pub reqwest::blocking::Client);

impl ReqwestHttp {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("HTTP client");
        Self(client)
    }
}

impl Http for ReqwestHttp {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
        let body: String = form.iter().map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v))).collect::<Vec<_>>().join("&");
        let resp = self
            .0
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        Ok((status, resp.text().map_err(|e| e.to_string())?))
    }
}

/// `xdg-open`, which hands the URL to the desktop's default browser.
pub struct XdgOpen;

impl Browser for XdgOpen {
    fn open(&self, url: &str) -> Result<(), String> {
        let status = std::process::Command::new("xdg-open")
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() { Ok(()) } else { Err(format!("xdg-open exited with {status}")) }
    }
}

/// `application/x-www-form-urlencoded` encoding of one key or value.
pub fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn random_b64(n: usize) -> String {
    let mut buf = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).expect("/dev/urandom");
    URL_SAFE_NO_PAD.encode(buf)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Lowercase a GUID client id; other ids (MSA app ids) stay as they are.
pub fn normalize_client_id(id: &str) -> String {
    if GUID.is_match(id) { id.to_ascii_lowercase() } else { id.to_string() }
}

/// The client id of a request that names its caller instead.
pub fn client_for_caller(caller: &str) -> Option<&'static str> {
    let caller = caller.to_ascii_lowercase();
    CALLER_CLIENT_IDS.iter().find(|(c, _)| *c == caller).map(|(_, id)| *id)
}

/// The tenant of an authority given either as a tenant name or as a login URL.
pub fn tenant_of(authority: &str, default: &str) -> Result<String, AuthError> {
    let value = if authority.is_empty() { default.to_string() } else { authority.to_ascii_lowercase() };
    if value.len() > 128 || value.bytes().any(|b| !(33..=126).contains(&b)) {
        return Err(invalid("authority"));
    }
    let tenant = if let Some(rest) = value.strip_prefix("https://") {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        if !["login.microsoftonline.com", "login.windows.net", "login.microsoft.com"].contains(&host)
            || path.contains(['?', '#', ';'])
        {
            return Err(invalid("authority"));
        }
        let t = path.strip_suffix('/').unwrap_or(path);
        if t.contains('/') {
            return Err(invalid("authority"));
        }
        t.to_string()
    } else {
        value
    };
    if !TENANT.is_match(&tenant) {
        return Err(invalid("authority"));
    }
    Ok(tenant)
}

fn printable(s: &str, max: usize) -> bool {
    s.len() <= max && s.is_ascii() && !s.bytes().any(|b| b < 32 || b == 127)
}

/// The scopes to request: the OpenID scopes first, then the caller's, with legacy names
/// translated. Resources a public client cannot get a token for are an error: Office asks
/// for telemetry interactively at every start, and that must not open the browser.
pub fn normalize_scopes(scopes: &str) -> Result<String, AuthError> {
    let requested: Vec<&str> = scopes.split_whitespace().collect();
    if requested.iter().any(|s| UNSUPPORTED_RESOURCES.contains(s)) {
        return Err(failed("unsupported identity resource"));
    }
    let mut out: Vec<&str> = vec!["openid", "profile", "email", "offline_access"];
    for s in requested {
        let s = SCOPE_ALIASES.iter().find(|(from, _)| *from == s).map(|(_, to)| *to).unwrap_or(s);
        if !out.contains(&s) {
            out.push(s);
        }
    }
    let joined = out.join(" ");
    if out.len() > 36 || joined.len() > 4096 || out.iter().any(|s| s.len() > 512 || !printable(s, 512) || s.contains(' ')) {
        return Err(invalid("scope"));
    }
    Ok(joined)
}

/// `scopes` without the resources a public client cannot get a token for.
fn supported_only(scopes: &str) -> String {
    scopes.split_whitespace().filter(|s| !UNSUPPORTED_RESOURCES.contains(s)).collect::<Vec<_>>().join(" ")
}

fn decode_claims(id_token: &str) -> Result<serde_json::Value, AuthError> {
    let payload = id_token.split('.').nth(1).ok_or_else(|| failed("invalid ID token"))?;
    let bytes = URL_SAFE_NO_PAD_INDIFFERENT.decode(payload).map_err(|_| failed("invalid ID token"))?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| failed("invalid ID token"))?;
    if !claims.is_object() {
        return Err(failed("invalid ID token"));
    }
    Ok(claims)
}

/// Whether an ID token's audience is `client_id`. Personal-account tokens for MSA app ids
/// (16 hex digits) carry the id as `00000000-0000-0000-xxxx-xxxxxxxxxxxx`.
pub fn audience_matches(aud: &serde_json::Value, client_id: &str, tenant: &str) -> bool {
    let Some(aud) = aud.as_str() else { return false };
    if aud == client_id {
        return true;
    }
    tenant == PERSONAL_TENANT
        && client_id.len() == 16
        && client_id.bytes().all(|b| b.is_ascii_hexdigit())
        && aud.eq_ignore_ascii_case(&format!("00000000-0000-0000-{}-{}", &client_id[..4], &client_id[4..]))
}

/// Parse a token endpoint response into a result.
fn result_from_token(token: &serde_json::Value, client_id: &str, nonce: Option<&str>) -> Result<AuthResult, AuthError> {
    let s = |k: &str| token.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (access, id_token, refresh) = (s("access_token"), s("id_token"), s("refresh_token"));
    if access.is_empty() || id_token.is_empty() || refresh.is_empty() {
        return Err(failed("incomplete token response"));
    }
    let claims = decode_claims(&id_token)?;
    let c = |k: &str| claims.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tenant = c("tid");
    if !audience_matches(&claims["aud"], client_id, &tenant) {
        return Err(failed("unexpected ID token audience"));
    }
    if let Some(n) = nonce
        && c("nonce") != n
    {
        return Err(failed("unexpected ID token nonce"));
    }
    let expires_in = token.get("expires_in").and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())).unwrap_or(0);
    if expires_in == 0 {
        return Err(failed("invalid token lifetime"));
    }
    let account_id = Some(c("oid")).filter(|v| !v.is_empty()).unwrap_or_else(|| c("sub"));
    let username = Some(c("preferred_username")).filter(|v| !v.is_empty()).unwrap_or_else(|| c("email"));
    if account_id.is_empty() || username.is_empty() {
        return Err(failed("incomplete account identity"));
    }
    let display = Some(c("name")).filter(|v| !v.is_empty()).unwrap_or_else(|| username.clone());
    Ok(AuthResult {
        access_token: access,
        expires_at: now() + expires_in,
        scopes: s("scope"),
        account: Record {
            refresh_token: refresh,
            id_token,
            account_id,
            username,
            first_name: c("given_name"),
            last_name: c("family_name"),
            display_name: display,
            tenant_id: tenant,
            client_info: s("client_info"),
        },
    })
}

/// What Microsoft returned to the redirect URI.
#[derive(Debug, PartialEq, Eq)]
pub struct Callback {
    pub state: String,
    pub client_id: String,
    pub code: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

pub fn parse_callback(uri: &str) -> Result<Callback, AuthError> {
    let url = reqwest::Url::parse(uri).map_err(|_| invalid("identity callback"))?;
    if !url.scheme().eq_ignore_ascii_case(CALLBACK_SCHEME)
        || !url.host_str().is_some_and(|h| h.eq_ignore_ascii_case(CALLBACK_HOST))
        || url.fragment().is_some()
    {
        return Err(invalid("identity callback"));
    }
    let mut q: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in url.query_pairs().take(16) {
        q.entry(k.into_owned()).or_default().push(v.into_owned());
    }
    let one = |k: &str| -> Result<Option<String>, AuthError> {
        match q.get(k).map(Vec::as_slice) {
            None => Ok(None),
            Some([v]) => Ok(Some(v.clone())),
            Some(_) => Err(invalid("identity callback")),
        }
    };
    let state = one("state")?.filter(|s| STATE.is_match(s)).ok_or_else(|| invalid("identity callback state"))?;
    let (code, error) = (one("code")?, one("error")?);
    if code.is_some() == error.is_some() {
        return Err(invalid("identity callback result"));
    }
    Ok(Callback {
        state,
        client_id: url.path().trim_start_matches('/').to_ascii_lowercase(),
        code,
        error,
        error_description: one("error_description")?,
    })
}

/// Where the sign-in for `state` waits for its callback.
pub fn callback_socket(dir: &Path, state: &str) -> PathBuf {
    let hash = hex::encode(Sha256::digest(state.as_bytes()));
    dir.join(format!("cb-{}.sock", &hash[..32]))
}

/// Hand a redirect URI from the browser to the sign-in waiting for it.
pub fn forward_callback(dir: &Path, uri: &str) -> Result<(), String> {
    if uri.len() > MAX_CALLBACK {
        return Err("the sign-in callback is too long".into());
    }
    let cb = parse_callback(uri).map_err(|e| e.to_string())?;
    let mut s = UnixStream::connect(callback_socket(dir, &cb.state))
        .map_err(|e| format!("no sign-in is waiting for this callback ({e})"))?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(&(uri.len() as u32).to_le_bytes()).and_then(|_| s.write_all(uri.as_bytes())).map_err(|e| e.to_string())?;
    let mut ack = [0u8; 1];
    s.read_exact(&mut ack).map_err(|e| e.to_string())?;
    if ack[0] == 1 { Ok(()) } else { Err("the waiting sign-in rejected the callback".into()) }
}

/// UID of the process on the other end of a Unix socket.
pub fn peer_uid(fd: &impl AsRawFd) -> Option<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: valid socket fd and a correctly sized out buffer.
    let rc = unsafe {
        libc::getsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    (rc == 0).then_some(cred.uid)
}

/// Wait until `fd` is readable or `ms` passes. True when readable (or hung up).
pub fn readable(fd: &impl AsRawFd, ms: i32) -> bool {
    let mut p = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // SAFETY: one valid pollfd.
    unsafe { libc::poll(&mut p, 1, ms) > 0 }
}

/// The socket a sign-in listens on for its callback; removed on drop.
struct CallbackListener {
    listener: UnixListener,
    path: PathBuf,
}

impl CallbackListener {
    fn bind(dir: &Path, state: &str) -> std::io::Result<Self> {
        let path = callback_socket(dir, state);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        Ok(Self { listener, path })
    }

    /// Accept callbacks until one matches `state` and `client_id`.
    fn wait(&self, state: &str, client_id: &str, cancelled: &dyn Fn() -> bool) -> Result<Callback, AuthError> {
        let deadline = Instant::now() + SIGN_IN_TIMEOUT;
        while Instant::now() < deadline {
            if cancelled() {
                return Err(AuthError::Canceled);
            }
            if !readable(&self.listener, 500) {
                continue;
            }
            let Ok((mut conn, _)) = self.listener.accept() else { continue };
            // SAFETY: geteuid has no preconditions.
            if peer_uid(&conn) != Some(unsafe { libc::geteuid() }) {
                continue;
            }
            conn.set_read_timeout(Some(Duration::from_secs(2))).ok();
            let mut len = [0u8; 4];
            let parsed = conn
                .read_exact(&mut len)
                .ok()
                .map(|_| u32::from_le_bytes(len) as usize)
                .filter(|&n| n > 0 && n <= MAX_CALLBACK)
                .and_then(|n| {
                    let mut buf = vec![0u8; n];
                    conn.read_exact(&mut buf).ok()?;
                    String::from_utf8(buf).ok()
                })
                .and_then(|uri| parse_callback(&uri).ok())
                .filter(|cb| cb.state == state && cb.client_id == client_id.to_ascii_lowercase());
            let _ = conn.write_all(&[parsed.is_some() as u8]);
            if let Some(cb) = parsed {
                return Ok(cb);
            }
        }
        Err(failed("sign-in timed out"))
    }
}

impl Drop for CallbackListener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

type CacheKey = (String, String, String, String);

pub struct Provider<K, H, B> {
    pub store: Store<K>,
    http: H,
    browser: B,
    callback_dir: PathBuf,
    cache: Mutex<HashMap<CacheKey, AuthResult>>,
}

impl<K: Kv, H: Http, B: Browser> Provider<K, H, B> {
    pub fn new(store: Store<K>, http: H, browser: B, callback_dir: PathBuf) -> Self {
        Self { store, http, browser, callback_dir, cache: Mutex::new(HashMap::new()) }
    }

    fn validate(client_id: &str, login_hint: &str) -> Result<(), AuthError> {
        if !CLIENT_ID.is_match(client_id) {
            return Err(invalid("client ID"));
        }
        if !printable(login_hint, 320) {
            return Err(invalid("login hint"));
        }
        Ok(())
    }

    fn request_token(&self, tenant: &str, form: &[(&str, &str)]) -> Result<serde_json::Value, AuthError> {
        let url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token");
        let mut form = form.to_vec();
        form.push(("client_info", "1"));
        let (status, body) = self.http.post_form(&url, &form).map_err(|e| failed(format!("token endpoint failed: {e}")))?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| failed("token endpoint failed"))?;
        if status != 200 || !value.is_object() {
            let msg = value.get("error_description").and_then(|v| v.as_str()).unwrap_or("token endpoint rejected the request");
            return Err(failed(msg));
        }
        Ok(value)
    }

    /// Sign in. Without `force`, a stored account is used silently when it still works.
    ///
    /// Office asks for its telemetry resource both automatically at every start and after
    /// the user enters an email in its Sign in window. Only the second carries a login hint:
    /// that one becomes a plain account sign-in, the first fails without a browser.
    pub fn authorize(
        &self,
        client_id: &str,
        tenant: &str,
        scopes: &str,
        login_hint: &str,
        force: bool,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AuthResult, AuthError> {
        let scopes = match normalize_scopes(scopes) {
            Err(AuthError::Failed(_)) if !login_hint.is_empty() => normalize_scopes(&supported_only(scopes))?,
            other => other?,
        };
        Self::validate(client_id, login_hint)?;
        let client_id = normalize_client_id(client_id);
        if !force {
            match self.get_token(&client_id, tenant, &scopes, "", login_hint) {
                Ok(r) => return Ok(r),
                Err(e @ AuthError::Storage(_)) => return Err(e),
                Err(_) => {}
            }
        }
        let verifier = random_b64(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_b64(32);
        let nonce = random_b64(32);
        let listener = CallbackListener::bind(&self.callback_dir, &state)
            .map_err(|e| failed(format!("unable to prepare the sign-in callback: {e}")))?;
        let redirect = format!("{CALLBACK_SCHEME}://{CALLBACK_HOST}/{}", client_id.to_ascii_lowercase());
        let mut params = vec![
            ("client_id", client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect.as_str()),
            ("response_mode", "query"),
            ("scope", scopes.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("client_info", "1"),
        ];
        if !login_hint.is_empty() {
            params.push(("login_hint", login_hint));
        }
        if force {
            params.push(("prompt", "login"));
        }
        let query: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", form_encode(v))).collect();
        let url = format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize?{}", query.join("&"));
        log::info!("opening the browser for Microsoft sign-in");
        self.browser.open(&url).map_err(|e| failed(format!("unable to open the system browser: {e}")))?;
        let cb = listener.wait(&state, &client_id, cancelled)?;
        if let Some(err) = cb.error {
            return Err(failed(cb.error_description.unwrap_or(err)));
        }
        let code = cb.code.ok_or_else(|| failed("authorization returned no code"))?;
        let token = self.request_token(
            tenant,
            &[
                ("client_id", &client_id),
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("code_verifier", &verifier),
                ("scope", &scopes),
            ],
        )?;
        let result = result_from_token(&token, &client_id, Some(&nonce))?;
        if cancelled() {
            return Err(AuthError::Canceled);
        }
        self.store.save(&client_id, &result.account)?;
        self.remember(&client_id, tenant, &scopes, &result);
        log::info!("signed in as {}", result.account.username);
        Ok(result)
    }

    /// A token from the stored refresh token of `account_id` (the current account if empty).
    pub fn get_token(
        &self,
        client_id: &str,
        tenant: &str,
        scopes: &str,
        account_id: &str,
        login_hint: &str,
    ) -> Result<AuthResult, AuthError> {
        let scopes = normalize_scopes(scopes)?;
        Self::validate(client_id, login_hint)?;
        let client_id = normalize_client_id(client_id);
        let stored = self.store.load(&client_id, account_id)?.ok_or_else(|| failed("account not found"))?;
        if !login_hint.is_empty() && !login_hint.eq_ignore_ascii_case(&stored.username) {
            return Err(failed("a different account was requested"));
        }
        let key = (client_id.clone(), tenant.to_string(), scopes.clone(), stored.account_id.clone());
        if let Some(hit) = self.cache.lock().unwrap().get(&key)
            && hit.expires_at > now() + 300
        {
            return Ok(hit.clone());
        }
        let mut token = self.request_token(
            tenant,
            &[
                ("client_id", &client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", &stored.refresh_token),
                ("scope", &scopes),
            ],
        )?;
        let obj = token.as_object_mut().expect("checked in request_token");
        obj.entry("refresh_token").or_insert_with(|| stored.refresh_token.clone().into());
        obj.entry("id_token").or_insert_with(|| stored.id_token.clone().into());
        let result = result_from_token(&token, &client_id, None)?;
        self.store.save(&client_id, &result.account)?;
        self.remember(&client_id, tenant, &scopes, &result);
        Ok(result)
    }

    fn remember(&self, client_id: &str, tenant: &str, scopes: &str, r: &AuthResult) {
        let key = (client_id.to_string(), tenant.to_string(), scopes.to_string(), r.account.account_id.clone());
        self.cache.lock().unwrap().insert(key, r.clone());
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::identity::store::MemKv;
    use std::sync::Arc;

    pub const OFFICE: &str = "d3590ed6-52b3-4102-aeff-aad2292ab01c";

    pub fn id_token(claims: serde_json::Value) -> String {
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    pub type Form = Vec<(String, String)>;

    /// Answers every token request with a fixed account (and the nonce of the last
    /// sign-in), and records the forms.
    #[derive(Default, Clone)]
    pub struct FakeHttp {
        pub forms: Arc<Mutex<Vec<Form>>>,
        pub nonce: Arc<Mutex<String>>,
    }

    impl Http for FakeHttp {
        fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
            assert!(url.starts_with("https://login.microsoftonline.com/"), "{url}");
            self.forms.lock().unwrap().push(form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
            let nonce = self.nonce.lock().unwrap().clone();
            Ok((
                200,
                serde_json::json!({
                    "access_token": "at",
                    "refresh_token": "rt2",
                    "expires_in": 3600,
                    "scope": "openid",
                    "id_token": id_token(serde_json::json!({
                        "aud": OFFICE, "tid": "t", "oid": "o1", "preferred_username": "u@example.com",
                        "name": "U", "nonce": nonce,
                    })),
                })
                .to_string(),
            ))
        }
    }

    /// Records the opened URL and plays the browser: it sends the redirect back.
    #[derive(Default, Clone)]
    pub struct FakeBrowser {
        pub dir: PathBuf,
        pub opened: Arc<Mutex<Vec<String>>>,
        pub nonce: Arc<Mutex<String>>,
    }

    impl Browser for FakeBrowser {
        fn open(&self, url: &str) -> Result<(), String> {
            self.opened.lock().unwrap().push(url.to_string());
            let u = reqwest::Url::parse(url).unwrap();
            let get = |k: &str| u.query_pairs().find(|(q, _)| q == k).unwrap().1.into_owned();
            *self.nonce.lock().unwrap() = get("nonce");
            let back = format!("{}?code=c0de&state={}", get("redirect_uri"), get("state"));
            let dir = self.dir.clone();
            std::thread::spawn(move || forward_callback(&dir, &back).unwrap());
            Ok(())
        }
    }

    pub fn provider(dir: &Path) -> Provider<MemKv, FakeHttp, FakeBrowser> {
        let nonce = Arc::new(Mutex::new(String::new()));
        let browser = FakeBrowser { dir: dir.to_path_buf(), nonce: nonce.clone(), ..Default::default() };
        let http = FakeHttp { nonce, ..Default::default() };
        Provider::new(Store::new(MemKv::default()), http, browser, dir.to_path_buf())
    }

    #[test]
    fn normalizes_scopes() {
        assert_eq!(normalize_scopes("").unwrap(), "openid profile email offline_access");
        assert_eq!(
            normalize_scopes("service::https://licensing.m365.svc.cloud.microsoft/::MBI_SSL_SHORT openid").unwrap(),
            "openid profile email offline_access https://licensing.m365.svc.cloud.microsoft/.default"
        );
        let telemetry = "https://events.data.microsoft.com/OneCollector/1.0/";
        assert!(matches!(normalize_scopes(telemetry), Err(AuthError::Failed(_))));
        assert!(matches!(normalize_scopes(&format!("{telemetry} openid")), Err(AuthError::Failed(_))));
    }

    #[test]
    fn parses_authorities() {
        assert_eq!(tenant_of("", "organizations").unwrap(), "organizations");
        assert_eq!(tenant_of("https://login.microsoftonline.com/common/", "x").unwrap(), "common");
        assert_eq!(tenant_of("Consumers", "x").unwrap(), "consumers");
        assert!(tenant_of("https://evil.example/common", "x").is_err());
        assert!(tenant_of("https://login.microsoftonline.com/common?x=1", "x").is_err());
        assert!(tenant_of("../etc", "x").is_err());
    }

    #[test]
    fn parses_callbacks() {
        let state = "a".repeat(43);
        let cb = parse_callback(&format!("ms-appx-web://microsoft.aad.brokerplugin/{OFFICE}?code=x%20y&state={state}")).unwrap();
        assert_eq!((cb.code.as_deref(), cb.client_id.as_str()), (Some("x y"), OFFICE));
        let err = parse_callback(&format!("ms-appx-web://microsoft.aad.brokerplugin/{OFFICE}?error=access_denied&state={state}")).unwrap();
        assert_eq!(err.error.as_deref(), Some("access_denied"));
        assert!(parse_callback(&format!("https://microsoft.aad.brokerplugin/x?code=1&state={state}")).is_err());
        assert!(parse_callback(&format!("ms-appx-web://other/x?code=1&state={state}")).is_err());
        assert!(parse_callback("ms-appx-web://microsoft.aad.brokerplugin/x?code=1&state=short").is_err());
        assert!(parse_callback(&format!("ms-appx-web://microsoft.aad.brokerplugin/x?code=1&error=e&state={state}")).is_err());
    }

    #[test]
    fn matches_audiences() {
        let a = |v: &str| serde_json::Value::String(v.into());
        assert!(audience_matches(&a(OFFICE), OFFICE, "t"));
        assert!(!audience_matches(&a("other"), OFFICE, "t"));
        assert!(audience_matches(&a("00000000-0000-0000-0000-00004C1A2B3C"), "000000004c1a2b3c", PERSONAL_TENANT));
        assert!(!audience_matches(&a("00000000-0000-0000-0000-00004C1A2B3C"), "000000004c1a2b3c", "t"));
    }

    #[test]
    fn encodes_forms() {
        assert_eq!(form_encode("a b+c/d~"), "a+b%2Bc%2Fd~");
    }

    #[test]
    fn signs_in_through_the_browser_then_refreshes_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let p = provider(tmp.path());
        let r = p.authorize(OFFICE, "common", "", "", false, &|| false).unwrap();
        assert_eq!(r.account.username, "u@example.com");
        let opened = p.browser.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains("code_challenge_method=S256"));
        assert!(opened[0].contains("redirect_uri=ms-appx-web%3A%2F%2Fmicrosoft.aad.brokerplugin%2Fd3590ed6"));
        let forms = p.http.forms.lock().unwrap().clone();
        assert!(forms[0].contains(&("grant_type".into(), "authorization_code".into())));
        assert!(forms[0].contains(&("code".into(), "c0de".into())));
        assert_eq!(p.store.load(OFFICE, "").unwrap().unwrap().refresh_token, "rt2");
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none(), "the callback socket is removed");

        // A second sign-in uses the stored account without the browser.
        p.authorize(OFFICE, "common", "", "", false, &|| false).unwrap();
        assert_eq!(p.browser.opened.lock().unwrap().len(), 1);
        // A forced one always opens the browser.
        p.authorize(OFFICE, "common", "", "", true, &|| false).unwrap();
        assert_eq!(p.browser.opened.lock().unwrap().len(), 2);
        assert!(p.browser.opened.lock().unwrap()[1].contains("prompt=login"));
    }

    /// What Office sends about 10 s after every start: an interactive request for telemetry.
    #[test]
    fn telemetry_authorize_never_opens_the_browser() {
        let tmp = tempfile::tempdir().unwrap();
        let p = provider(tmp.path());
        let telemetry = "https://events.data.microsoft.com/OneCollector/1.0/";
        assert!(matches!(p.authorize(OFFICE, "common", telemetry, "", false, &|| false), Err(AuthError::Failed(_))));
        assert!(p.browser.opened.lock().unwrap().is_empty());
    }

    /// What Office sends after the user enters an email in its Sign in window.
    #[test]
    fn telemetry_authorize_with_an_email_signs_in() {
        let tmp = tempfile::tempdir().unwrap();
        let p = provider(tmp.path());
        let telemetry = "https://events.data.microsoft.com/OneCollector/1.0/";
        let r = p.authorize(OFFICE, "common", telemetry, "u@example.com", false, &|| false).unwrap();
        assert_eq!(r.account.username, "u@example.com");
        let opened = p.browser.opened.lock().unwrap().clone();
        assert_eq!(opened.len(), 1);
        assert!(opened[0].contains("scope=openid+profile+email+offline_access&"), "{}", opened[0]);
        assert!(opened[0].contains("login_hint=u%40example.com"));
        assert!(!opened[0].contains("events.data"));
        // Silent requests for telemetry still fail.
        assert!(matches!(p.get_token(OFFICE, "common", telemetry, "", ""), Err(AuthError::Failed(_))));
    }

    #[test]
    fn get_token_needs_a_stored_account() {
        let tmp = tempfile::tempdir().unwrap();
        let p = provider(tmp.path());
        assert!(matches!(p.get_token(OFFICE, "common", "", "", ""), Err(AuthError::Failed(_))));
        assert!(matches!(p.get_token("bad id!", "common", "", "", ""), Err(AuthError::Invalid(_))));
    }

    #[test]
    fn rejects_a_callback_for_another_sign_in() {
        let tmp = tempfile::tempdir().unwrap();
        let state = "s".repeat(43);
        let l = CallbackListener::bind(tmp.path(), &state).unwrap();
        let other = format!("ms-appx-web://microsoft.aad.brokerplugin/{OFFICE}?code=1&state={}", "o".repeat(43));
        assert!(forward_callback(tmp.path(), &other).is_err(), "no listener for that state");
        let dir = tmp.path().to_path_buf();
        let right = format!("ms-appx-web://microsoft.aad.brokerplugin/{OFFICE}?code=1&state={state}");
        let t = std::thread::spawn(move || forward_callback(&dir, &right));
        let cb = l.wait(&state, OFFICE, &|| false).unwrap();
        assert_eq!(cb.code.as_deref(), Some("1"));
        assert!(t.join().unwrap().is_ok());
        let cancelled = l.wait(&state, OFFICE, &|| true);
        assert!(matches!(cancelled, Err(AuthError::Canceled)));
    }
}
