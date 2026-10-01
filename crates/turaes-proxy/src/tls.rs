//! Certificate discovery for the `certbot` layout.
//!
//! turaes does **not** perform ACME in-process. Certificates are issued by
//! certbot (the existing fleet workflow) into
//! `{cert_dir}/{domain}/{fullchain.pem,privkey.pem}`; the proxy loads them and
//! reloads gracefully when files change (M2).

use std::path::{Path, PathBuf};

/// Paths to a domain's certificate chain and private key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertPaths {
    /// `fullchain.pem`.
    pub fullchain: PathBuf,
    /// `privkey.pem`.
    pub private_key: PathBuf,
}

/// Resolves certificate paths under a certbot live directory.
#[derive(Debug, Clone)]
pub struct CertStore {
    root: PathBuf,
}

impl CertStore {
    /// Create a store rooted at `root` (e.g. `/etc/letsencrypt/live`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Compute the expected cert paths for a domain.
    pub fn paths(&self, domain: &str) -> CertPaths {
        let dir = self.root.join(domain);
        CertPaths {
            fullchain: dir.join("fullchain.pem"),
            private_key: dir.join("privkey.pem"),
        }
    }

    /// Whether both files exist on disk.
    pub fn has_cert(&self, domain: &str) -> bool {
        let p = self.paths(domain);
        p.fullchain.is_file() && p.private_key.is_file()
    }

    /// Domains present on disk (directories containing both PEM files).
    pub fn domains(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                if let Some(name) = entry.file_name().to_str() {
                    if self.has_cert(name) {
                        out.push(name.to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Root directory of the store.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_certbot_layout() {
        let store = CertStore::new("/etc/letsencrypt/live");
        let p = store.paths("kalkulator.rayakala.ink");
        assert_eq!(
            p.fullchain,
            PathBuf::from("/etc/letsencrypt/live/kalkulator.rayakala.ink/fullchain.pem")
        );
        assert_eq!(
            p.private_key,
            PathBuf::from("/etc/letsencrypt/live/kalkulator.rayakala.ink/privkey.pem")
        );
    }

    #[test]
    fn has_cert_requires_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::new(dir.path());
        assert!(!store.has_cert("a.example.com"));
        let domain_dir = dir.path().join("a.example.com");
        std::fs::create_dir_all(&domain_dir).unwrap();
        std::fs::write(domain_dir.join("fullchain.pem"), "cert").unwrap();
        assert!(!store.has_cert("a.example.com"));
        std::fs::write(domain_dir.join("privkey.pem"), "key").unwrap();
        assert!(store.has_cert("a.example.com"));
        assert_eq!(store.domains(), vec!["a.example.com".to_string()]);
    }
}
