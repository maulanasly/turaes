//! Cryptographic primitives: session JWTs and secret-at-rest sealing.
//!
//! - **Sessions** are stateless JWTs (`HS256`) stored in an HttpOnly cookie,
//!   signed with the configured secret.
//! - **App environment variables** (and sealed agent/SSH material) use
//!   AES-256-GCM with a key derived from the configured secret via
//!   HKDF-SHA256 under a dedicated info string — never the raw signing
//!   secret, and never the same key as any other use.
//! - Sealed blobs carry a version prefix (`v1$…`). Unprefixed blobs are the
//!   pre-HKDF legacy format (key = SHA-256 of the secret) and still decrypt
//!   via a fallback cipher; `turaes secrets reseal` migrates them.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Claims embedded in a session JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// GitHub numeric user id (as a string subject).
    pub sub: String,
    /// GitHub login.
    pub login: String,
    /// Display name, when available.
    pub name: Option<String>,
    /// Issued-at (unix seconds).
    pub iat: i64,
    /// Expiry (unix seconds).
    pub exp: i64,
}

/// Mints and verifies session tokens.
#[derive(Clone)]
pub struct TokenIssuer {
    key: Vec<u8>,
    ttl_days: i64,
}

impl TokenIssuer {
    /// Create an issuer from a signing secret and session TTL.
    pub fn new(secret: &str, ttl_days: i64) -> Self {
        Self {
            key: secret.as_bytes().to_vec(),
            ttl_days,
        }
    }

    /// Mint a fresh token for a signed-in user.
    pub fn mint(&self, id: i64, login: &str, name: Option<&str>) -> Result<String> {
        let now = chrono::Utc::now();
        let claims = Claims {
            sub: id.to_string(),
            login: login.to_string(),
            name: name.map(str::to_string),
            iat: now.timestamp(),
            exp: (now + chrono::Duration::days(self.ttl_days)).timestamp(),
        };
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(&self.key),
        )
        .map_err(|e| Error::Internal(format!("failed to mint token: {e}")))
    }

    /// Verify a token and return its claims.
    pub fn verify(&self, token: &str) -> Result<Claims> {
        let data = jsonwebtoken::decode::<Claims>(
            token,
            &jsonwebtoken::DecodingKey::from_secret(&self.key),
            &jsonwebtoken::Validation::default(),
        )
        .map_err(|_| Error::Unauthorized("invalid or expired session".into()))?;
        Ok(data.claims)
    }
}

/// Domain separation label for the at-rest sealing key.
const SEAL_INFO: &[u8] = b"turaes/env-seal/v1";

/// Prefix marking the current sealed-blob format.
const SEAL_VERSION_PREFIX: &str = "v1$";

/// Derive a 32-byte sealing key from the configured secret via HKDF-SHA256.
fn sealing_key(secret: &str) -> [u8; 32] {
    let hk = hkdf::Hkdf::<Sha256>::new(None, secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(SEAL_INFO, &mut key)
        .expect("HKDF-SHA256 expands to 32 bytes");
    key
}

/// Legacy (pre-HKDF) key: SHA-256 of the secret, kept as a decrypt-only
/// fallback until `turaes secrets reseal` migrates every stored value.
fn legacy_key(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn cipher_for(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("32-byte key")
}

/// Seals and opens small secrets with AES-256-GCM.
///
/// The primary cipher derives from the configured secret via HKDF-SHA256, so
/// the at-rest key is independent of the JWT signing key. `with_previous`
/// adds the previous secret's ciphers as decrypt-only fallbacks for rotation;
/// `open` transparently handles both the current (`v1$…`) and legacy
/// (unprefixed) blob formats.
#[derive(Clone)]
pub struct SecretBox {
    /// Ciphers for `v1$` blobs, primary first.
    primary: Vec<Aes256Gcm>,
    /// Ciphers for unprefixed legacy blobs, newest first.
    legacy: Vec<Aes256Gcm>,
}

impl SecretBox {
    /// Derive ciphers from the configured secret.
    pub fn new(secret: &str) -> Self {
        Self {
            primary: vec![cipher_for(&sealing_key(secret))],
            legacy: vec![cipher_for(&legacy_key(secret))],
        }
    }

    /// Derive ciphers from the current secret plus a previous secret that is
    /// being rotated out (its ciphers decrypt, never encrypt).
    pub fn with_previous(secret: &str, previous: &str) -> Self {
        Self {
            primary: vec![
                cipher_for(&sealing_key(secret)),
                cipher_for(&sealing_key(previous)),
            ],
            legacy: vec![
                cipher_for(&legacy_key(secret)),
                cipher_for(&legacy_key(previous)),
            ],
        }
    }

    /// Encrypt UTF-8 plaintext into a versioned `v1$base64(nonce||ciphertext)`
    /// blob using the primary cipher.
    pub fn seal(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self.primary[0]
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| Error::Internal("failed to seal secret".into()))?;
        let mut out = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(format!(
            "{SEAL_VERSION_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(out)
        ))
    }

    /// Encrypt with the legacy (unprefixed) format. Migration tooling and
    /// tests only — everything else must use [`SecretBox::seal`].
    pub fn seal_legacy(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self.legacy[0]
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| Error::Internal("failed to seal secret".into()))?;
        let mut out = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(out))
    }

    /// True for pre-HKDF blobs (no version prefix): still decryptable, but a
    /// `turaes secrets reseal` away from the current format.
    pub fn is_legacy_format(sealed: &str) -> bool {
        !sealed.starts_with(SEAL_VERSION_PREFIX)
    }

    /// Decrypt a value produced by [`SecretBox::seal`] or the legacy format.
    pub fn open(&self, sealed: &str) -> Result<String> {
        let (ciphers, body) = match sealed.strip_prefix(SEAL_VERSION_PREFIX) {
            Some(body) => (&self.primary, body),
            None => (&self.legacy, sealed),
        };
        let raw = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| Error::Internal("invalid sealed secret encoding".into()))?;
        if raw.len() < 13 {
            return Err(Error::Internal("sealed secret too short".into()));
        }
        let (nonce_bytes, ciphertext) = raw.split_at(12);
        for cipher in ciphers {
            if let Ok(plaintext) = cipher.decrypt(Nonce::from_slice(nonce_bytes), ciphertext) {
                return String::from_utf8(plaintext)
                    .map_err(|_| Error::Internal("sealed secret is not valid UTF-8".into()));
            }
        }
        Err(Error::Internal("failed to open secret".into()))
    }
}

/// Generate a URL-safe random token of `bytes` entropy (CSRF/OAuth state).
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// Lowercase hex SHA-256 of a token (used to look up an agent by its token
/// without storing the token in plaintext).
pub fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_roundtrip() {
        let issuer = TokenIssuer::new("a-test-secret-that-is-long-enough-123", 7);
        let token = issuer.mint(42, "octocat", Some("Mona")).unwrap();
        let claims = issuer.verify(&token).unwrap();
        assert_eq!(claims.sub, "42");
        assert_eq!(claims.login, "octocat");
        assert_eq!(claims.name.as_deref(), Some("Mona"));
    }

    #[test]
    fn jwt_rejects_wrong_secret() {
        let a = TokenIssuer::new("secret-a-very-long-string-aaaaaaaaaa", 1);
        let b = TokenIssuer::new("secret-b-very-long-string-bbbbbbbbbb", 1);
        let token = a.mint(1, "x", None).unwrap();
        assert!(b.verify(&token).is_err());
    }

    #[test]
    fn seal_roundtrip() {
        let sbox = SecretBox::new("master-secret");
        let sealed = sbox.seal("postgres://secret").unwrap();
        assert!(sealed.starts_with("v1$"));
        assert_ne!(sealed, "postgres://secret");
        assert_eq!(sbox.open(&sealed).unwrap(), "postgres://secret");
    }

    #[test]
    fn seal_key_differs_from_signing_key_material() {
        // The at-rest key must not be the raw secret (JWT HMAC key) nor its
        // plain SHA-256 (the legacy format): HKDF domain separation.
        let derived = sealing_key("master-secret");
        assert_ne!(derived, legacy_key("master-secret"));
        assert_ne!(derived.as_slice(), b"master-secret".as_slice());
        let again = sealing_key("master-secret");
        assert_eq!(derived, again);
        assert_ne!(derived, sealing_key("other-secret"));
    }

    #[test]
    fn legacy_blobs_still_open() {
        let sbox = SecretBox::new("master-secret");
        let legacy = sbox.seal_legacy("postgres://legacy").unwrap();
        assert!(SecretBox::is_legacy_format(&legacy));
        assert!(!SecretBox::is_legacy_format(&sbox.seal("x").unwrap()));
        assert_eq!(sbox.open(&legacy).unwrap(), "postgres://legacy");
    }

    #[test]
    fn rotation_opens_old_and_seals_new() {
        let old_box = SecretBox::new("old-secret");
        let legacy_blob = old_box.seal_legacy("a").unwrap();
        let v1_blob = old_box.seal("b").unwrap();

        let rotated = SecretBox::with_previous("new-secret", "old-secret");
        // Old blobs (both formats) still decrypt via fallbacks…
        assert_eq!(rotated.open(&legacy_blob).unwrap(), "a");
        assert_eq!(rotated.open(&v1_blob).unwrap(), "b");
        // …but fresh seals use the new primary and the old box cannot read them.
        let fresh = rotated.seal("c").unwrap();
        assert_eq!(rotated.open(&fresh).unwrap(), "c");
        assert!(old_box.open(&fresh).is_err());
    }

    #[test]
    fn seal_same_plaintext_differs() {
        let sbox = SecretBox::new("master-secret");
        assert_ne!(sbox.seal("x").unwrap(), sbox.seal("x").unwrap());
    }

    #[test]
    fn random_tokens_are_unique() {
        assert_ne!(random_token(16), random_token(16));
    }

    #[test]
    fn token_hash_is_stable_hex() {
        let h = token_hash("abc");
        assert_eq!(h.len(), 64);
        assert_eq!(h, token_hash("abc"));
        assert_ne!(h, token_hash("abd"));
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
