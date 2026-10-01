//! Cryptographic primitives: session JWTs and secret-at-rest sealing.
//!
//! - **Sessions** are stateless JWTs (`HS256`) stored in an HttpOnly cookie.
//! - **App environment variables** are sealed with AES-256-GCM. The 32-byte key
//!   is derived from the configured secret via SHA-256, so operators only need
//!   to manage one string.

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

/// Seals and opens small secrets with AES-256-GCM.
#[derive(Clone)]
pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    /// Derive the cipher from the configured secret.
    pub fn new(secret: &str) -> Self {
        let digest = Sha256::digest(secret.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&digest).expect("32-byte key");
        Self { cipher }
    }

    /// Encrypt UTF-8 plaintext into a URL-safe base64 `nonce||ciphertext`.
    pub fn seal(&self, plaintext: &str) -> Result<String> {
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| Error::Internal("failed to seal secret".into()))?;
        let mut out = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(out))
    }

    /// Decrypt a value produced by [`SecretBox::seal`].
    pub fn open(&self, sealed: &str) -> Result<String> {
        let raw = URL_SAFE_NO_PAD
            .decode(sealed)
            .map_err(|_| Error::Internal("invalid sealed secret encoding".into()))?;
        if raw.len() < 13 {
            return Err(Error::Internal("sealed secret too short".into()));
        }
        let (nonce_bytes, ciphertext) = raw.split_at(12);
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|_| Error::Internal("failed to open secret".into()))?;
        String::from_utf8(plaintext)
            .map_err(|_| Error::Internal("sealed secret is not valid UTF-8".into()))
    }
}

/// Generate a URL-safe random token of `bytes` entropy (CSRF/OAuth state).
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
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
        assert_ne!(sealed, "postgres://secret");
        assert_eq!(sbox.open(&sealed).unwrap(), "postgres://secret");
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
}
