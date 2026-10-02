//! Content-addressed artifact store.
//!
//! Deploys record an artifact hash and the bytes live under
//! `{root}/sha256/{hex}`. This decouples "where the binary came from" from
//! "which node runs it": rollback reuses a previous hash, and (N1) remote
//! agents fetch artifacts by hash over the control-plane channel.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{Error, Result};

/// A filesystem-backed, content-addressed artifact store.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    /// Create a store rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Root directory of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate/normalise a hash, returning the bare hex (no `sha256:` prefix).
    fn hex(hash: &str) -> Result<&str> {
        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        let ok = hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit());
        if ok {
            Ok(hex)
        } else {
            Err(Error::BadRequest(format!("invalid artifact hash '{hash}'")))
        }
    }

    /// Absolute path for a stored artifact.
    pub fn path_for(&self, hash: &str) -> Result<PathBuf> {
        Ok(self.root.join("sha256").join(Self::hex(hash)?))
    }

    /// Whether the artifact for `hash` is present.
    pub fn has(&self, hash: &str) -> bool {
        Self::hex(hash)
            .ok()
            .map(|hex| self.root.join("sha256").join(hex).is_file())
            .unwrap_or(false)
    }

    /// Store the file at `src`, returning its `sha256:<hex>` hash.
    ///
    /// Streams the file (hash + temp copy), then atomically renames it into
    /// place. Idempotent: an existing artifact is reused.
    pub async fn put_file(&self, src: &Path) -> Result<String> {
        let dir = self.root.join("sha256");
        tokio::fs::create_dir_all(&dir).await?;

        let mut input = tokio::fs::File::open(src).await.map_err(|e| {
            Error::BadRequest(format!("cannot read artifact '{}': {e}", src.display()))
        })?;
        let tmp = dir.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
        let mut output = tokio::fs::File::create(&tmp).await?;

        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            output.write_all(&buf[..n]).await?;
        }
        output.flush().await?;
        drop(output);

        // Preserve the source's permission bits (executable binaries must stay
        // executable). `File::create` would otherwise drop them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = tokio::fs::metadata(src).await {
                let mode = meta.permissions().mode() & 0o7777;
                if let Ok(mut perms) = tokio::fs::metadata(&tmp).await.map(|m| m.permissions()) {
                    perms.set_mode(mode);
                    let _ = tokio::fs::set_permissions(&tmp, perms).await;
                }
            }
        }

        let hash = format!("sha256:{}", to_hex(&hasher.finalize()));
        let dest = self.path_for(&hash)?;
        if dest.exists() {
            let _ = tokio::fs::remove_file(&tmp).await;
        } else {
            tokio::fs::rename(&tmp, &dest).await?;
        }
        Ok(hash)
    }
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path());
        let src = dir.path().join("bin");
        tokio::fs::write(&src, b"hello world").await.unwrap();

        let hash = store.put_file(&src).await.unwrap();
        assert!(hash.starts_with("sha256:"));
        assert_eq!(hash.len(), "sha256:".len() + 64);
        assert!(store.has(&hash));
        assert!(store.has(&hash["sha256:".len()..]));
        let path = store.path_for(&hash).unwrap();
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"hello world");

        // Idempotent: storing the same bytes yields the same hash.
        let again = store.put_file(&src).await.unwrap();
        assert_eq!(hash, again);
    }

    #[tokio::test]
    async fn rejects_bad_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path());
        assert!(store.path_for("sha256:nothex").is_err());
        assert!(store.path_for("../../etc/passwd").is_err());
    }

    #[tokio::test]
    async fn missing_source_is_bad_request() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::new(dir.path());
        let err = store
            .put_file(Path::new("/no/such/file"))
            .await
            .unwrap_err();
        matches!(err, Error::BadRequest(_));
    }
}
