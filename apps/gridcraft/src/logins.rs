//! Remembered database logins (Data › From Database › Remember), kept in the system's credential
//! store through the `keyring` crate: Windows Credential Manager, the macOS keychain, and on Linux
//! and the BSDs the Secret Service (GNOME Keyring, KWallet). Each login is one entry named after
//! its server; one more entry lists the servers, since the stores can't be listed. GridCraft
//! writes nothing to disk itself. Without a Secret Service running, remembering a login fails
//! with the store's message and logins last the session.

use gridcraft_engine::datasource::{Credential, Kind, LoginStore, login_key};
use serde_json::{Value, json};

const SERVICE: &str = "GridCraft database logins";
/// The entry that lists the servers with a remembered login.
const INDEX: &str = "(servers)";

pub struct KeyringStore;

fn entry(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, name).map_err(|e| e.to_string())
}

impl KeyringStore {
    fn index(&self) -> Vec<String> {
        entry(INDEX).and_then(|e| e.get_password().map_err(|e| e.to_string())).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn set_index(&self, keys: &[String]) -> Result<(), String> {
        let e = entry(INDEX)?;
        if keys.is_empty() {
            return match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
        }
        e.set_password(&json!(keys).to_string()).map_err(|e| e.to_string())
    }
}

/// The kind, host and port of a [`login_key`] (`kind:host:port`; a host may hold colons).
fn parse_key(key: &str) -> Option<(Kind, String, Option<u16>)> {
    let (kind, rest) = key.split_once(':')?;
    let (host, port) = rest.rsplit_once(':')?;
    let kind = Kind::parse(kind)?;
    let port: u16 = port.parse().ok()?;
    Some((kind, host.to_string(), (Some(port) != kind.default_port()).then_some(port)))
}

impl LoginStore for KeyringStore {
    fn load(&self) -> Result<Vec<Credential>, String> {
        let mut out = Vec::new();
        for key in self.index() {
            let Some((kind, host, port)) = parse_key(&key) else { continue };
            let Ok(secret) = entry(&key).and_then(|e| e.get_password().map_err(|e| e.to_string())) else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&secret) else { continue };
            out.push(Credential {
                kind,
                host,
                port,
                username: v["username"].as_str().unwrap_or("").to_string(),
                password: v["password"].as_str().unwrap_or("").to_string(),
                encrypt: v["encrypt"].as_bool().unwrap_or(true),
                trust_server_certificate: v["trust"].as_bool().unwrap_or(false),
            });
        }
        Ok(out)
    }

    fn save(&self, c: &Credential) -> Result<(), String> {
        let key = login_key(c.kind, &c.host, c.port);
        let secret = json!({"username": c.username, "password": c.password, "encrypt": c.encrypt, "trust": c.trust_server_certificate});
        entry(&key)?.set_password(&secret.to_string()).map_err(|e| e.to_string())?;
        let mut keys = self.index();
        if !keys.contains(&key) {
            keys.push(key);
            self.set_index(&keys)?;
        }
        Ok(())
    }

    fn remove(&self, kind: Kind, host: &str, port: Option<u16>) -> Result<(), String> {
        let key = login_key(kind, host, port);
        match entry(&key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut keys = self.index();
        keys.retain(|k| k != &key);
        self.set_index(&keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_back() {
        assert_eq!(parse_key(&login_key(Kind::SqlServer, "DB.local", Some(1500))), Some((Kind::SqlServer, "db.local".into(), Some(1500))));
        assert_eq!(parse_key(&login_key(Kind::PostgreSql, "::1", None)), Some((Kind::PostgreSql, "::1".into(), None)));
        assert_eq!(parse_key("nonsense"), None);
    }
}
