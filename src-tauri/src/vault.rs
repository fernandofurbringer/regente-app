use crate::error::{AppError, AppResult};
use crate::settings::save_json;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SERVICE: &str = "Regente";
const USER: &str = "vault-master";

#[derive(Serialize, Deserialize, Default)]
struct VaultFile {
    version: u32,
    items: BTreeMap<String, Item>,
}

#[derive(Serialize, Deserialize)]
struct Item {
    n: String,
    d: String,
}

pub struct Vault {
    path: PathBuf,
    key: [u8; 32],
}

pub(crate) fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> AppResult<Vec<u8>> {
    if !s.is_ascii() || s.len() % 2 != 0 {
        return Err(unreadable("hex inválido"));
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| unreadable("hex inválido"))).collect()
}

fn unreadable(why: impl std::fmt::Display) -> AppError {
    AppError::new("vault_unreadable", format!("Não foi possível ler o Cofre: {why}"))
}

/// Nome de segredo com escopo de workspace: `ws:<id>:<nome>`. Id e nome: letras, números, `-`, `_` (e `.` no nome), até 64.
pub fn scoped_name(workspace_id: &str, name: &str) -> AppResult<String> {
    let part = |s: &str, extra: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c));
    if !part(workspace_id, "-_") || !part(name, "-_.") {
        return Err(AppError::new("bad_name", "segredo por workspace: id e nome só com letras, números, '-', '_' e '.' (nome até 64)"));
    }
    Ok(format!("ws:{workspace_id}:{name}"))
}

/// Pura: o nome é do escopo de um workspace? Devolve (id, nome). Nome global nunca começa com `ws:`.
pub fn split_scoped(name: &str) -> Option<(&str, &str)> {
    let (id, n) = name.strip_prefix("ws:")?.split_once(':')?;
    (!id.is_empty() && !n.is_empty()).then_some((id, n))
}

/// Pura: um nome que vai ao cofre é válido (global livre, ou escopado com partes válidas).
pub fn check_name(name: &str) -> AppResult<()> {
    match split_scoped(name) {
        Some((id, n)) => scoped_name(id, n).map(|_| ()),
        None if name.starts_with("ws:") => Err(AppError::new("bad_name", "nome escopado: use ws:<id>:<nome>")),
        None => Ok(()),
    }
}

pub(crate) fn random<const N: usize>() -> AppResult<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| AppError::new("rng", e.to_string()))?;
    Ok(b)
}

impl Vault {
    /// Master key in the Windows Credential Manager (created on first use).
    pub fn open(path: &Path) -> AppResult<Self> {
        let kerr = |e: keyring::Error| AppError::new("keyring", e.to_string());
        let entry = keyring::Entry::new(SERVICE, USER).map_err(kerr)?;
        let key: [u8; 32] = match entry.get_password() {
            Ok(h) => from_hex(&h)?.try_into().map_err(|_| unreadable("chave-mestra com tamanho inválido"))?,
            Err(keyring::Error::NoEntry) => {
                let k = random::<32>()?;
                entry.set_password(&to_hex(&k)).map_err(kerr)?;
                k
            }
            Err(e) => return Err(kerr(e)),
        };
        Ok(Self::with_key(path, key))
    }

    pub fn with_key(path: &Path, key: [u8; 32]) -> Self {
        Self { path: path.to_path_buf(), key }
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.key).expect("32-byte key")
    }

    fn load(&self) -> AppResult<VaultFile> {
        if !self.path.exists() {
            return Ok(VaultFile { version: 2, ..Default::default() });
        }
        let s = std::fs::read_to_string(&self.path).map_err(unreadable)?;
        serde_json::from_str(&s).map_err(unreadable)
    }

    pub fn names(&self) -> AppResult<Vec<String>> {
        Ok(self.load()?.items.into_keys().collect())
    }

    pub fn set(&self, name: &str, value: &str) -> AppResult<()> {
        let mut f = self.load()?; // never overwrites an unreadable file
        let n = random::<12>()?;
        let nonce = Nonce::try_from(&n[..]).expect("12-byte nonce");
        let d = self.cipher().encrypt(&nonce, Payload { msg: value.as_bytes(), aad: name.as_bytes() }).map_err(|e| AppError::new("crypto", e.to_string()))?;
        f.items.insert(name.to_string(), Item { n: to_hex(&n), d: to_hex(&d) });
        f.version = 2;
        save_json(&self.path, &f)
    }

    /// For Rust-internal use only (never exposed to the UI).
    pub fn get(&self, name: &str) -> AppResult<Option<String>> {
        let f = self.load()?;
        let Some(it) = f.items.get(name) else { return Ok(None) };
        let n = from_hex(&it.n)?;
        let nonce = Nonce::try_from(&n[..]).map_err(|_| unreadable("nonce inválido"))?;
        let pt = self.cipher().decrypt(&nonce, Payload { msg: &from_hex(&it.d)?, aad: name.as_bytes() }).map_err(|_| unreadable("chave incorreta ou dado corrompido"))?;
        Ok(Some(String::from_utf8(pt).map_err(unreadable)?))
    }

    pub fn delete(&self, name: &str) -> AppResult<()> {
        let mut f = self.load()?;
        f.items.remove(name);
        save_json(&self.path, &f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_names_are_validated_and_split_back() {
        assert_eq!(scoped_name("ab-12", "route.openrouter").unwrap(), "ws:ab-12:route.openrouter");
        assert_eq!(split_scoped("ws:ab-12:route.openrouter"), Some(("ab-12", "route.openrouter")));
        assert!(scoped_name("a:b", "x").is_err(), "colon would break the split");
        assert!(scoped_name("", "x").is_err());
        assert!(scoped_name("ok", "bad name").is_err());
        assert!(scoped_name("ok", &"n".repeat(65)).is_err());
        assert_eq!(split_scoped("route.openrouter"), None);
        assert_eq!(split_scoped("ws::x"), None);
        assert!(check_name("ws:ab:x").is_ok() && check_name("global.name").is_ok());
        assert!(check_name("ws:sem-partes").is_err());
        assert!(check_name("ws:a:b:c").is_err(), "a colon in the name is refused");
    }

    #[test]
    fn roundtrip_and_delete() {
        let d = tempfile::tempdir().unwrap();
        let v = Vault::with_key(&d.path().join("vault.json"), [7u8; 32]);
        v.set("route.openrouter", "sk-or-123").unwrap();
        assert_eq!(v.get("route.openrouter").unwrap().as_deref(), Some("sk-or-123"));
        assert_eq!(v.names().unwrap(), ["route.openrouter"]);
        assert!(!std::fs::read_to_string(d.path().join("vault.json")).unwrap().contains("sk-or-123"));
        v.delete("route.openrouter").unwrap();
        assert_eq!(v.get("route.openrouter").unwrap(), None);
    }

    #[test]
    fn wrong_key_fails() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.json");
        Vault::with_key(&p, [1u8; 32]).set("a", "b").unwrap();
        assert_eq!(Vault::with_key(&p, [2u8; 32]).get("a").unwrap_err().code(), "vault_unreadable");
    }

    #[test]
    fn non_ascii_hex_does_not_panic() {
        assert_eq!(from_hex("é").unwrap_err().code(), "vault_unreadable");
        assert_eq!(from_hex("abc").unwrap_err().code(), "vault_unreadable");
    }

    #[test]
    fn swapped_items_fail() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.json");
        let v = Vault::with_key(&p, [3u8; 32]);
        v.set("a", "um").unwrap();
        v.set("b", "dois").unwrap();
        let mut j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let (ia, ib) = (j["items"]["a"].clone(), j["items"]["b"].clone());
        j["items"]["a"] = ib;
        j["items"]["b"] = ia;
        std::fs::write(&p, j.to_string()).unwrap();
        assert_eq!(v.get("a").unwrap_err().code(), "vault_unreadable");
    }

    #[test]
    fn corrupted_file_is_preserved() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.json");
        std::fs::write(&p, "{lixo").unwrap();
        let v = Vault::with_key(&p, [1u8; 32]);
        assert_eq!(v.names().unwrap_err().code(), "vault_unreadable");
        assert!(v.set("a", "b").is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{lixo");
    }
}
