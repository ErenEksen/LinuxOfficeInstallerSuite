//! The bridge server: answers the Soda runner's identity requests on a Unix socket.
//!
//! Requests are handled on their own threads, so account lookups keep working while a
//! browser sign-in waits. A sign-in is cancelled when Wine closes its connection. The
//! server exits after ten idle minutes once no process uses its socket any more.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use regex::Regex;

use super::protocol::*;
use super::provider::{
    AuthError, AuthResult, Browser, Http, PERSONAL_TENANT, Provider, client_for_caller, normalize_client_id, peer_uid,
    readable, tenant_of,
};
use super::store::Kv;

const IDLE_EXIT: Duration = Duration::from_secs(600);
pub const SOCKET_ENV: &str = "SODA_IDENTITY_BRIDGE_SOCKET";

static CLIENT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9.-]{1,128}$").unwrap());

fn printable(s: &str, max: usize) -> bool {
    s.len() <= max && s.is_ascii() && !s.bytes().any(|b| b < 32 || b == 127)
}

fn bad(what: &str) -> AuthError {
    AuthError::Invalid(what.to_string())
}

fn text(req: &Message, id: u16) -> Result<String, AuthError> {
    req.text(id).map_err(|e| AuthError::Invalid(e.to_string()))
}

fn required(req: &Message, id: u16) -> Result<String, AuthError> {
    Some(text(req, id)?).filter(|s| !s.is_empty()).ok_or_else(|| bad("missing required field"))
}

fn result_fields(r: &AuthResult) -> BTreeMap<u16, Vec<u8>> {
    let a = &r.account;
    [
        (FIELD_SCOPES, r.scopes.as_str()),
        (FIELD_ACCESS_TOKEN, &r.access_token),
        (FIELD_ID_TOKEN, &a.id_token),
        (FIELD_EXPIRES_AT, &r.expires_at.to_string()),
        (FIELD_ACCOUNT_ID, &a.account_id),
        (FIELD_USERNAME, &a.username),
        (FIELD_FIRST_NAME, &a.first_name),
        (FIELD_LAST_NAME, &a.last_name),
        (FIELD_DISPLAY_NAME, &a.display_name),
        (FIELD_TENANT_ID, &a.tenant_id),
        (FIELD_CLIENT_INFO, &a.client_info),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.as_bytes().to_vec()))
    .collect()
}

/// One log line for a request and its reply: what was asked and how it ended. Tokens are
/// never included, only the request's names and the reply's status, error and user name.
fn describe(req: &Message, reply: &Message) -> String {
    let op = match req.opcode {
        OP_AUTHORIZE => "authorize",
        OP_GET_TOKEN => "get_token",
        OP_CLEAR => "clear",
        OP_LIST_ACCOUNTS => "list_accounts",
        _ => "unknown",
    };
    let mut line = format!("{op} #{}", req.request_id);
    for (id, name) in [(FIELD_CLIENT_ID, "client"), (FIELD_CALLER_ID, "caller"), (FIELD_AUTHORITY, "authority"), (FIELD_SCOPES, "scopes"), (FIELD_PROMPT, "prompt")] {
        if let Some(v) = req.fields.get(&id) {
            line.push_str(&format!(" {name}={:?}", String::from_utf8_lossy(v)));
        }
    }
    if req.fields.get(&FIELD_LOGIN_HINT).is_some_and(|v| !v.is_empty()) {
        line.push_str(" login_hint=yes");
    }
    line.push_str(&format!(" -> status {}", reply.status));
    if let Some(v) = reply.fields.get(&FIELD_USERNAME) {
        line.push_str(&format!(" user={:?}", String::from_utf8_lossy(v)));
    }
    if let Some(v) = reply.fields.get(&FIELD_ACCOUNTS) {
        let count = v.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0);
        line.push_str(&format!(" accounts={count}"));
    }
    if let Some(v) = reply.fields.get(&FIELD_ERROR) {
        line.push_str(&format!(" error={:?}", String::from_utf8_lossy(v)));
    }
    line
}

/// Whether an account of `tenant` belongs to `authority`.
fn in_authority(authority: &str, tenant: &str) -> bool {
    let tenant = tenant.to_ascii_lowercase();
    match authority {
        "common" => true,
        "consumers" => tenant == PERSONAL_TENANT,
        "organizations" => !tenant.is_empty() && tenant != PERSONAL_TENANT,
        t => tenant == t,
    }
}

pub struct Server<K, H, B> {
    pub provider: Provider<K, H, B>,
}

impl<K: Kv, H: Http, B: Browser> Server<K, H, B> {
    /// Answer one request. `cancelled` reports whether the client went away.
    pub fn dispatch(&self, req: &Message, cancelled: &dyn Fn() -> bool) -> Message {
        let reply = match self.handle(req, cancelled) {
            Ok((status, fields)) => Message::reply(req, status, fields),
            Err(e) => {
                let status = match e {
                    AuthError::Invalid(_) => STATUS_INVALID_REQUEST,
                    AuthError::Canceled => STATUS_CANCELED,
                    AuthError::Storage(_) | AuthError::Failed(_) => STATUS_AUTH_FAILED,
                };
                Message::reply(req, status, BTreeMap::from([(FIELD_ERROR, e.to_string().into_bytes())]))
            }
        };
        if req.opcode != OP_PING {
            log::info!("{}", describe(req, &reply));
        }
        reply
    }

    fn handle(&self, req: &Message, cancelled: &dyn Fn() -> bool) -> Result<(i32, BTreeMap<u16, Vec<u8>>), AuthError> {
        if req.status != 0 || req.opcode & RESPONSE_BIT != 0 {
            return Err(bad("invalid request header"));
        }
        let allowed: &[u16] = match req.opcode {
            OP_PING => &[],
            OP_AUTHORIZE => &[FIELD_CLIENT_ID, FIELD_CALLER_ID, FIELD_SCOPES, FIELD_AUTHORITY, FIELD_LOGIN_HINT, FIELD_POLICY, FIELD_PROMPT],
            OP_GET_TOKEN => &[FIELD_CLIENT_ID, FIELD_SCOPES, FIELD_AUTHORITY, FIELD_ACCOUNT_ID],
            OP_CLEAR => &[FIELD_CLIENT_ID, FIELD_ACCOUNT_ID],
            OP_LIST_ACCOUNTS => &[FIELD_CLIENT_ID, FIELD_AUTHORITY],
            _ => return Err(bad("unsupported operation")),
        };
        if req.fields.keys().any(|k| !allowed.contains(k)) {
            return Err(bad("unexpected field"));
        }
        let account_id = text(req, FIELD_ACCOUNT_ID)?;
        if !printable(&account_id, 256) {
            return Err(bad("invalid account ID"));
        }
        let store = &self.provider.store;
        match req.opcode {
            OP_PING => Ok((STATUS_OK, BTreeMap::new())),
            OP_LIST_ACCOUNTS => {
                let mut client = text(req, FIELD_CLIENT_ID)?;
                if !client.is_empty() && !CLIENT_ID.is_match(&client) {
                    return Err(bad("invalid client ID"));
                }
                if let Some(mapped) = client_for_caller(&client) {
                    client = mapped.to_string();
                }
                let authority = tenant_of(&text(req, FIELD_AUTHORITY)?, "common")?;
                let accounts: Vec<(String, String)> = store
                    .accounts(&normalize_client_id(&client))?
                    .into_iter()
                    .filter(|r| in_authority(&authority, &r.tenant_id))
                    .map(|r| (r.account_id, r.username))
                    .collect();
                Ok((STATUS_OK, BTreeMap::from([(FIELD_ACCOUNTS, encode_accounts(&accounts))])))
            }
            OP_AUTHORIZE => {
                let client = text(req, FIELD_CLIENT_ID)?;
                let caller = text(req, FIELD_CALLER_ID)?;
                let client = match (client.is_empty(), caller.is_empty()) {
                    (false, true) => client,
                    (true, false) => client_for_caller(&caller).ok_or_else(|| bad("unsupported caller ID"))?.to_string(),
                    _ => return Err(bad("exactly one identity field is required")),
                };
                let scopes = required(req, FIELD_SCOPES)?;
                if !printable(&text(req, FIELD_POLICY)?, 256) {
                    return Err(bad("invalid policy"));
                }
                let force = match text(req, FIELD_PROMPT)?.as_str() {
                    "" => false,
                    "login" => true,
                    _ => return Err(bad("invalid authentication prompt")),
                };
                let tenant = tenant_of(&text(req, FIELD_AUTHORITY)?, "organizations")?;
                let login_hint = text(req, FIELD_LOGIN_HINT)?;
                let r = self.provider.authorize(&client, &tenant, &scopes, &login_hint, force, cancelled)?;
                Ok((STATUS_OK, result_fields(&r)))
            }
            OP_GET_TOKEN => {
                let client = required(req, FIELD_CLIENT_ID)?;
                let tenant = tenant_of(&text(req, FIELD_AUTHORITY)?, "organizations")?;
                let r = self.provider.get_token(&client, &tenant, &required(req, FIELD_SCOPES)?, &account_id, "")?;
                Ok((STATUS_OK, result_fields(&r)))
            }
            OP_CLEAR => {
                let client = normalize_client_id(&required(req, FIELD_CLIENT_ID)?);
                let status = if store.clear(&client, &account_id)? { STATUS_OK } else { STATUS_NOT_FOUND };
                Ok((status, BTreeMap::new()))
            }
            _ => unreachable!("checked above"),
        }
    }
}

impl<K: Kv + 'static, H: Http + 'static, B: Browser + 'static> Server<K, H, B> {
    fn handle_connection(&self, mut conn: UnixStream) -> io::Result<()> {
        conn.set_read_timeout(Some(Duration::from_secs(30)))?;
        let req = Message::read(&mut conn).map_err(io::Error::other)?;
        // Wine sends nothing after its request, so a readable socket means it hung up.
        let cancelled = || readable(&conn, 0);
        let resp = self.dispatch(&req, &cancelled);
        if !cancelled() {
            conn.write_all(&resp.encode())?;
        }
        Ok(())
    }

    /// Serve on `socket` until idle. Returns at once when another bridge already answers there.
    pub fn serve(self: Arc<Self>, socket: &Path) -> io::Result<()> {
        if socket.exists() {
            if ping(socket) {
                log::info!("another identity bridge already serves {}", socket.display());
                return Ok(());
            }
            std::fs::remove_file(socket)?;
        }
        let listener = UnixListener::bind(socket)?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        log::info!("identity bridge listening on {}", socket.display());
        let start = Instant::now();
        let last = Arc::new(AtomicU64::new(0));
        // SAFETY: geteuid has no preconditions.
        let uid = unsafe { libc::geteuid() };
        loop {
            if !readable(&listener, 1000) {
                let idle = start.elapsed().saturating_sub(Duration::from_secs(last.load(Ordering::Relaxed)));
                if idle >= IDLE_EXIT {
                    if !has_clients(socket) {
                        break;
                    }
                    last.store(start.elapsed().as_secs(), Ordering::Relaxed);
                }
                continue;
            }
            let Ok((conn, _)) = listener.accept() else { continue };
            if peer_uid(&conn) != Some(uid) {
                continue;
            }
            let (server, last) = (self.clone(), last.clone());
            std::thread::spawn(move || {
                if let Err(e) = server.handle_connection(conn) {
                    log::debug!("identity connection: {e}");
                }
                last.store(start.elapsed().as_secs(), Ordering::Relaxed);
            });
        }
        log::info!("identity bridge idle; exiting");
        let _ = std::fs::remove_file(socket);
        Ok(())
    }
}

/// Whether a bridge answers on `socket`.
pub fn ping(socket: &Path) -> bool {
    let Ok(mut s) = UnixStream::connect(socket) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let id = std::process::id();
    let req = Message { opcode: OP_PING, request_id: id, ..Default::default() };
    if s.write_all(&req.encode()).is_err() {
        return false;
    }
    matches!(Message::read(&mut s), Ok(r) if r.opcode == OP_PING | RESPONSE_BIT && r.request_id == id && r.status == STATUS_OK)
}

/// Whether any process still has `SODA_IDENTITY_BRIDGE_SOCKET=<socket>` in its environment.
fn has_clients(socket: &Path) -> bool {
    let marker = format!("{SOCKET_ENV}={}", socket.display()).into_bytes();
    let me = std::process::id().to_string();
    let Ok(dir) = std::fs::read_dir("/proc") else { return false };
    dir.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        name.bytes().all(|b| b.is_ascii_digit())
            && name != me
            && std::fs::read(e.path().join("environ")).is_ok_and(|env| env.split(|&b| b == 0).any(|v| v == marker))
    })
}

/// Start a bridge for `socket` in the background unless one answers already, and wait
/// until it does. `serve` is the command that runs [`Server::serve`].
pub fn ensure(socket: &Path, serve: &mut std::process::Command) -> io::Result<PathBuf> {
    if ping(socket) {
        return Ok(socket.to_path_buf());
    }
    use std::os::unix::process::CommandExt;
    let mut child = serve.stdin(std::process::Stdio::null()).process_group(0).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if ping(socket) {
            // Reap it in the background so it does not linger as a zombie of a short-lived parent.
            std::thread::spawn(move || child.wait());
            return Ok(socket.to_path_buf());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(io::Error::other(format!("the identity bridge exited with {status}")));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(io::Error::other("the identity bridge did not start"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::provider::tests::{OFFICE, provider};

    fn req(opcode: u16, fields: &[(u16, &str)]) -> Message {
        Message {
            opcode,
            request_id: 9,
            status: 0,
            fields: fields.iter().map(|(k, v)| (*k, v.as_bytes().to_vec())).collect(),
        }
    }

    #[test]
    fn answers_ping_and_rejects_bad_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Server { provider: provider(tmp.path()) };
        let r = s.dispatch(&req(OP_PING, &[]), &|| false);
        assert_eq!((r.opcode, r.request_id, r.status), (OP_PING | RESPONSE_BIT, 9, STATUS_OK));
        assert_eq!(s.dispatch(&req(OP_PING, &[(FIELD_SCOPES, "x")]), &|| false).status, STATUS_INVALID_REQUEST);
        assert_eq!(s.dispatch(&req(99, &[]), &|| false).status, STATUS_INVALID_REQUEST);
        let both = req(OP_AUTHORIZE, &[(FIELD_CLIENT_ID, OFFICE), (FIELD_CALLER_ID, "x"), (FIELD_SCOPES, "openid")]);
        assert_eq!(s.dispatch(&both, &|| false).status, STATUS_INVALID_REQUEST);
        let unknown = req(OP_AUTHORIZE, &[(FIELD_CALLER_ID, "nobody"), (FIELD_SCOPES, "openid")]);
        assert_eq!(s.dispatch(&unknown, &|| false).status, STATUS_INVALID_REQUEST);
        let get = s.dispatch(&req(OP_GET_TOKEN, &[(FIELD_CLIENT_ID, OFFICE), (FIELD_SCOPES, "openid")]), &|| false);
        assert_eq!(get.status, STATUS_AUTH_FAILED, "no stored account yet");
        assert_eq!(get.text(FIELD_ERROR).unwrap(), "account not found");
    }

    #[test]
    fn signs_in_by_caller_id_and_lists_the_account() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Server { provider: provider(tmp.path()) };
        let auth = req(
            OP_AUTHORIZE,
            &[(FIELD_CALLER_ID, "2B379600-B42B-4FE9-A59C-A312FB934935"), (FIELD_SCOPES, "openid"), (FIELD_AUTHORITY, "https://login.microsoftonline.com/common")],
        );
        let r = s.dispatch(&auth, &|| false);
        assert_eq!(r.status, STATUS_OK, "{:?}", r.text(FIELD_ERROR));
        assert_eq!(r.text(FIELD_USERNAME).unwrap(), "u@example.com");
        assert_eq!(r.text(FIELD_ACCESS_TOKEN).unwrap(), "at");
        assert!(r.text(FIELD_EXPIRES_AT).unwrap().parse::<u64>().is_ok());

        let list = s.dispatch(&req(OP_LIST_ACCOUNTS, &[(FIELD_CLIENT_ID, OFFICE)]), &|| false);
        assert_eq!(list.fields[&FIELD_ACCOUNTS], encode_accounts(&[("o1".into(), "u@example.com".into())]));
        let consumers = s.dispatch(&req(OP_LIST_ACCOUNTS, &[(FIELD_AUTHORITY, "consumers")]), &|| false);
        assert_eq!(consumers.fields[&FIELD_ACCOUNTS], encode_accounts(&[]), "a work account is not a personal one");

        let clear = s.dispatch(&req(OP_CLEAR, &[(FIELD_CLIENT_ID, OFFICE)]), &|| false);
        assert_eq!(clear.status, STATUS_OK);
        assert_eq!(s.dispatch(&req(OP_CLEAR, &[(FIELD_CLIENT_ID, OFFICE)]), &|| false).status, STATUS_NOT_FOUND);
    }

    #[test]
    fn logs_requests_without_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let s = Server { provider: provider(tmp.path()) };
        let auth = req(OP_AUTHORIZE, &[(FIELD_CLIENT_ID, OFFICE), (FIELD_SCOPES, "openid")]);
        let line = describe(&auth, &s.dispatch(&auth, &|| false));
        assert!(line.starts_with("authorize #9 client=\"d3590ed6"), "{line}");
        assert!(line.contains("-> status 0 user=\"u@example.com\""), "{line}");
        assert!(!line.contains("rt2") && !line.contains("\"at\""), "no tokens: {line}");
        let list = req(OP_LIST_ACCOUNTS, &[]);
        assert!(describe(&list, &s.dispatch(&list, &|| false)).ends_with("accounts=1"));
    }

    #[test]
    fn filters_accounts_by_authority() {
        assert!(in_authority("common", "x"));
        assert!(in_authority("consumers", PERSONAL_TENANT));
        assert!(!in_authority("organizations", PERSONAL_TENANT));
        assert!(in_authority("organizations", "abc"));
        assert!(!in_authority("organizations", ""));
    }

    #[test]
    fn serves_over_a_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("bridge.sock");
        let server = Arc::new(Server { provider: provider(tmp.path()) });
        let path = socket.clone();
        std::thread::spawn(move || server.serve(&path).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ping(&socket) {
            assert!(Instant::now() < deadline, "bridge did not come up");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(socket.metadata().unwrap().permissions().mode() & 0o777, 0o600);

        let mut s = UnixStream::connect(&socket).unwrap();
        s.write_all(&req(OP_AUTHORIZE, &[(FIELD_CLIENT_ID, OFFICE), (FIELD_SCOPES, "openid")]).encode()).unwrap();
        let r = Message::read(&mut s).unwrap();
        assert_eq!(r.status, STATUS_OK, "{:?}", r.text(FIELD_ERROR));

        // A second server on the same socket leaves the first alone.
        let again = Arc::new(Server { provider: provider(tmp.path()) });
        again.serve(&socket).unwrap();
        assert!(ping(&socket));
    }
}
