//! Rails' encrypted cookies, as `ActionDispatch::Cookies`'s encrypted jar
//! writes them with Rails 8.1's defaults: AES-256-GCM under a key PBKDF2-
//! SHA256 derives from `secret_key_base` (salt "authenticated encrypted
//! cookie", 1000 iterations), the value wrapped as
//! `{"_rails":{"message":BASE64,"exp":null,"pur":"cookie.NAME"}}`, and
//! written as `base64(ciphertext)--base64(iv)--base64(tag)`.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value as Json, json};
use sha2::Sha256;

/// The key Rails' key generator derives for encrypted cookies.
#[derive(Clone)]
pub struct CookieKey([u8; 32]);

impl std::fmt::Debug for CookieKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CookieKey(..)")
    }
}

impl CookieKey {
    pub fn derive(secret_key_base: &str) -> Self {
        let mut key = [0; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(secret_key_base.as_bytes(), b"authenticated encrypted cookie", 1000, &mut key);
        Self(key)
    }

    /// The cookie value for `plain` in the cookie `name`, before URL escaping.
    pub fn encrypt(&self, name: &str, plain: &[u8]) -> String {
        let envelope = json!({ "_rails": { "message": STANDARD.encode(plain), "exp": null, "pur": format!("cookie.{name}") } });
        let mut iv = [0; 12];
        getrandom::fill(&mut iv).expect("the OS has randomness");
        let cipher = Aes256Gcm::new_from_slice(&self.0).expect("a 32-byte key");
        let sealed = cipher.encrypt(Nonce::from_slice(&iv), envelope.to_string().as_bytes()).expect("AES-GCM encrypts any input");
        let (data, tag) = sealed.split_at(sealed.len() - 16);
        format!("{}--{}--{}", STANDARD.encode(data), STANDARD.encode(iv), STANDARD.encode(tag))
    }

    /// What `encrypt` sealed for the cookie `name`, or None when the value
    /// was tampered with, is for another cookie, or has expired, as Rails
    /// treats such a cookie as absent.
    pub fn decrypt(&self, name: &str, value: &str) -> Option<Vec<u8>> {
        let mut parts = value.split("--").map(|part| STANDARD.decode(part).ok());
        let (data, iv, tag) = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() || iv.len() != 12 || tag.len() != 16 {
            return None;
        }
        let cipher = Aes256Gcm::new_from_slice(&self.0).ok()?;
        let sealed = [data, tag].concat();
        let plain = cipher.decrypt(Nonce::from_slice(&iv), Payload { msg: &sealed, aad: b"" }).ok()?;
        let envelope: Json = serde_json::from_slice(&plain).ok()?;
        let rails = envelope.get("_rails")?;
        if rails.get("pur")?.as_str()? != format!("cookie.{name}") {
            return None;
        }
        if let Some(expires) = rails.get("exp").and_then(Json::as_str) {
            let expires = chrono::DateTime::parse_from_rfc3339(expires).ok()?;
            if expires < chrono::Utc::now() {
                return None;
            }
        }
        STANDARD.decode(rails.get("message")?.as_str()?).ok()
    }
}
