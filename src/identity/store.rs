//! Refresh tokens and account details, kept in the desktop keyring (Secret Service:
//! KWallet, GNOME Keyring, KeePassXC …), never in a file.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::core::paths::APP_ID;

const CURRENT: &str = "__current__";

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(pub String);

impl StoreError {
    fn unavailable(detail: impl std::fmt::Display) -> Self {
        log::warn!("secret service: {detail}");
        StoreError(
            "Secure credential storage is unavailable. Start or unlock a Secret Service provider, \
             such as KWallet or GNOME Keyring, and try again."
                .into(),
        )
    }
}

/// One signed-in account for one client id.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub refresh_token: String,
    pub id_token: String,
    pub account_id: String,
    pub username: String,
    pub first_name: String,
    pub last_name: String,
    pub display_name: String,
    pub tenant_id: String,
    pub client_info: String,
}

/// A stored secret with its attributes.
pub type Item = (HashMap<String, String>, String);

/// Attribute-addressed secret storage.
pub trait Kv: Send + Sync {
    fn put(&self, attrs: &[(&str, &str)], label: &str, value: &str) -> Result<(), StoreError>;
    fn search(&self, attrs: &[(&str, &str)]) -> Result<Vec<Item>, StoreError>;
    fn delete(&self, attrs: &[(&str, &str)]) -> Result<bool, StoreError>;
}

pub struct Store<K> {
    kv: K,
}

impl<K: Kv> Store<K> {
    pub fn new(kv: K) -> Self {
        Self { kv }
    }

    fn attrs<'a>(client: &'a str, account: &'a str) -> Vec<(&'a str, &'a str)> {
        let mut a = vec![("application", APP_ID), ("kind", "microsoft-identity")];
        if !client.is_empty() {
            a.push(("client", client));
        }
        if !account.is_empty() {
            a.push(("account", account));
        }
        a
    }

    fn get(&self, client: &str, account: &str) -> Result<Option<String>, StoreError> {
        Ok(self.kv.search(&Self::attrs(client, account))?.into_iter().next().map(|(_, v)| v))
    }

    /// Store `rec` and make it the current account of `client`.
    pub fn save(&self, client: &str, rec: &Record) -> Result<(), StoreError> {
        let value = serde_json::to_string(rec).map_err(|e| StoreError(e.to_string()))?;
        self.kv.put(&Self::attrs(client, &rec.account_id), "Microsoft account (Office in Wine)", &value)?;
        let current = serde_json::json!({ "account_id": rec.account_id }).to_string();
        self.kv.put(&Self::attrs(client, CURRENT), "Microsoft account selection (Office in Wine)", &current)
    }

    fn current(&self, client: &str) -> Result<Option<String>, StoreError> {
        let Some(v) = self.get(client, CURRENT)? else { return Ok(None) };
        let v: serde_json::Value = serde_json::from_str(&v).map_err(|e| StoreError(e.to_string()))?;
        Ok(v["account_id"].as_str().map(String::from))
    }

    /// The account `account_id` of `client`; an empty id means the current account. An id of
    /// the form `<object id>.<tenant id>` (MSAL home account id) also finds the account.
    pub fn load(&self, client: &str, account_id: &str) -> Result<Option<Record>, StoreError> {
        let id = if account_id.is_empty() {
            match self.current(client)? {
                Some(id) => id,
                None => return Ok(None),
            }
        } else {
            account_id.to_string()
        };
        let mut found = self.get(client, &id)?;
        let mut home = None;
        if found.is_none()
            && let Some((oid, tid)) = id.split_once('.')
            && !oid.is_empty()
            && !tid.is_empty()
            && !tid.contains('.')
        {
            home = Some((oid.to_ascii_lowercase(), tid.to_ascii_lowercase()));
            found = self.get(client, &oid.to_ascii_lowercase())?;
        }
        let Some(v) = found else { return Ok(None) };
        let rec: Record = serde_json::from_str(&v).map_err(|e| StoreError(format!("invalid stored account: {e}")))?;
        if rec.refresh_token.is_empty() {
            return Ok(None);
        }
        if let Some((oid, tid)) = home
            && (rec.account_id.to_ascii_lowercase() != oid || rec.tenant_id.to_ascii_lowercase() != tid)
        {
            return Ok(None);
        }
        Ok(Some(rec))
    }

    /// Every stored account of `client` (any client when empty), sorted by id.
    pub fn accounts(&self, client: &str) -> Result<Vec<Record>, StoreError> {
        let mut out: Vec<Record> = Vec::new();
        for (attrs, value) in self.kv.search(&Self::attrs(client, ""))? {
            if attrs.get("account").is_none_or(|a| a == CURRENT) {
                continue;
            }
            let Ok(rec) = serde_json::from_str::<Record>(&value) else { continue };
            if !rec.refresh_token.is_empty() && !out.iter().any(|r| r.account_id == rec.account_id) {
                out.push(rec);
            }
        }
        out.sort_by(|a, b| a.account_id.cmp(&b.account_id));
        Ok(out)
    }

    /// Forget an account (the current one when `account_id` is empty).
    pub fn clear(&self, client: &str, account_id: &str) -> Result<bool, StoreError> {
        let id = if account_id.is_empty() {
            match self.current(client)? {
                Some(id) => id,
                None => return Ok(false),
            }
        } else {
            account_id.to_string()
        };
        let cleared = self.kv.delete(&Self::attrs(client, &id))?;
        self.kv.delete(&Self::attrs(client, CURRENT))?;
        Ok(cleared)
    }
}

/// The session's Secret Service over D-Bus.
pub struct SecretKv;

impl SecretKv {
    fn with<T>(
        f: impl FnOnce(&secret_service::blocking::Collection) -> Result<T, secret_service::Error>,
    ) -> Result<T, StoreError> {
        use secret_service::EncryptionType;
        use secret_service::blocking::SecretService;
        let ss = SecretService::connect(EncryptionType::Dh).map_err(StoreError::unavailable)?;
        let collection = ss.get_any_collection().map_err(StoreError::unavailable)?;
        collection.ensure_unlocked().map_err(StoreError::unavailable)?;
        f(&collection).map_err(StoreError::unavailable)
    }
}

impl Kv for SecretKv {
    fn put(&self, attrs: &[(&str, &str)], label: &str, value: &str) -> Result<(), StoreError> {
        Self::with(|c| c.create_item(label, attrs.iter().copied().collect(), value.as_bytes(), true, "text/plain").map(|_| ()))
    }

    fn search(&self, attrs: &[(&str, &str)]) -> Result<Vec<Item>, StoreError> {
        Self::with(|c| {
            let mut out = Vec::new();
            for item in c.search_items(attrs.iter().copied().collect())? {
                item.ensure_unlocked()?;
                let value = String::from_utf8_lossy(&item.get_secret()?).into_owned();
                out.push((item.get_attributes()?, value));
            }
            Ok(out)
        })
    }

    fn delete(&self, attrs: &[(&str, &str)]) -> Result<bool, StoreError> {
        Self::with(|c| {
            let items = c.search_items(attrs.iter().copied().collect())?;
            let any = !items.is_empty();
            for item in items {
                item.delete()?;
            }
            Ok(any)
        })
    }
}

/// In-memory [`Kv`] for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemKv(pub std::sync::Mutex<Vec<Item>>);

#[cfg(test)]
impl MemKv {
    fn matches(item: &HashMap<String, String>, attrs: &[(&str, &str)]) -> bool {
        attrs.iter().all(|(k, v)| item.get(*k).map(String::as_str) == Some(*v))
    }
}

#[cfg(test)]
impl Kv for MemKv {
    fn put(&self, attrs: &[(&str, &str)], _label: &str, value: &str) -> Result<(), StoreError> {
        let mut items = self.0.lock().unwrap();
        let attrs: HashMap<String, String> = attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        items.retain(|(a, _)| *a != attrs);
        items.push((attrs, value.to_string()));
        Ok(())
    }

    fn search(&self, attrs: &[(&str, &str)]) -> Result<Vec<Item>, StoreError> {
        Ok(self.0.lock().unwrap().iter().filter(|(a, _)| Self::matches(a, attrs)).cloned().collect())
    }

    fn delete(&self, attrs: &[(&str, &str)]) -> Result<bool, StoreError> {
        let mut items = self.0.lock().unwrap();
        let before = items.len();
        items.retain(|(a, _)| !Self::matches(a, attrs));
        Ok(items.len() != before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, tenant: &str) -> Record {
        Record {
            refresh_token: format!("rt-{id}"),
            account_id: id.into(),
            username: format!("{id}@example.com"),
            tenant_id: tenant.into(),
            ..Default::default()
        }
    }

    #[test]
    fn saves_loads_and_clears_accounts() {
        let store = Store::new(MemKv::default());
        assert_eq!(store.load("c", "").unwrap(), None);
        store.save("c", &rec("a", "t1")).unwrap();
        store.save("c", &rec("b", "t2")).unwrap();
        assert_eq!(store.load("c", "").unwrap().unwrap().account_id, "b", "the last sign-in is current");
        assert_eq!(store.load("c", "a").unwrap().unwrap().refresh_token, "rt-a");
        assert_eq!(store.load("c", "a.t1").unwrap().unwrap().account_id, "a", "home account id");
        assert_eq!(store.load("c", "a.other").unwrap(), None, "home account id with the wrong tenant");
        assert_eq!(store.load("other-client", "a").unwrap(), None);
        let ids: Vec<_> = store.accounts("c").unwrap().into_iter().map(|r| r.account_id).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(store.accounts("").unwrap().len(), 2);

        assert!(store.clear("c", "").unwrap());
        assert_eq!(store.load("c", "b").unwrap(), None);
        assert_eq!(store.load("c", "").unwrap(), None);
        assert!(store.load("c", "a").unwrap().is_some());
    }
}
