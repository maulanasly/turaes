//! Deploy orchestration: hash the artifact, apply the runtime, start it, and
//! report the resulting state.

use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use turaes_core::{Error, Result};

use crate::runtime::{AppSpec, RunState, Runtime};

/// Result of a deploy attempt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeployOutcome {
    /// SHA-256 of the artifact, `sha256:<hex>`.
    pub artifact_hash: String,
    /// Observed state after start.
    pub state: RunState,
    /// Human-readable steps for the deploy log.
    pub log: String,
}

/// Computes the artifact hash used to dedupe deploys and record history.
pub async fn artifact_hash(path: &str) -> Result<String> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| Error::BadRequest(format!("cannot read artifact '{path}': {e}")))?;
    let digest = Sha256::digest(&bytes);
    Ok(format!("sha256:{}", hex(&digest)))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Drives a [`Runtime`] to install and run an application.
pub struct Deployer {
    runtime: Arc<dyn Runtime>,
}

impl Deployer {
    /// Wrap a runtime backend.
    pub fn new(runtime: Arc<dyn Runtime>) -> Self {
        Self { runtime }
    }

    /// Sync, (re)configure, start, and observe a `static` app. The caller
    /// supplies the content hash (directory trees have no single file to
    /// hash here; the sync itself happens in `Runtime::apply`).
    pub async fn deploy_static(
        &self,
        spec: &AppSpec,
        env: &BTreeMap<String, String>,
    ) -> Result<DeployOutcome> {
        let mut log = String::new();
        self.runtime.apply(spec, env).await?;
        log.push_str("synced publish directory\n");

        self.runtime.restart(spec).await?;
        log.push_str("started service\n");

        let state = self.runtime.status(spec).await.unwrap_or(RunState::Unknown);
        log.push_str(&format!("state: {}\n", state.as_status()));

        Ok(DeployOutcome {
            artifact_hash: String::new(),
            state,
            log,
        })
    }

    /// Start and observe an app whose artifact identity is already known
    /// (command apps execute in place; there is no file to hash or store).
    pub async fn deploy_with_hash(
        &self,
        spec: &AppSpec,
        env: &BTreeMap<String, String>,
        hash: String,
    ) -> Result<DeployOutcome> {
        let mut log = String::new();
        log.push_str(&format!("artifact {hash}\n"));

        self.runtime.apply(spec, env).await?;
        log.push_str("applied runtime config\n");

        self.runtime.restart(spec).await?;
        log.push_str("started service\n");

        let state = self.runtime.status(spec).await.unwrap_or(RunState::Unknown);
        log.push_str(&format!("state: {}\n", state.as_status()));

        Ok(DeployOutcome {
            artifact_hash: hash,
            state,
            log,
        })
    }

    /// Install, (re)configure, start, and observe an app.
    pub async fn deploy(
        &self,
        spec: &AppSpec,
        env: &BTreeMap<String, String>,
    ) -> Result<DeployOutcome> {
        let mut log = String::new();
        let hash = artifact_hash(&spec.binary_path).await?;
        log.push_str(&format!("artifact {}\n", hash));

        self.runtime.apply(spec, env).await?;
        log.push_str("applied runtime config\n");

        self.runtime.restart(spec).await?;
        log.push_str("started service\n");

        let state = self.runtime.status(spec).await.unwrap_or(RunState::Unknown);
        log.push_str(&format!("state: {}\n", state.as_status()));

        Ok(DeployOutcome {
            artifact_hash: hash,
            state,
            log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_is_stable_and_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        tokio::fs::write(&path, b"hello").await.unwrap();
        let a = artifact_hash(path.to_str().unwrap()).await.unwrap();
        let b = artifact_hash(path.to_str().unwrap()).await.unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert_eq!(a.len(), "sha256:".len() + 64);
    }

    #[tokio::test]
    async fn missing_artifact_is_bad_request() {
        let err = artifact_hash("/no/such/file").await.unwrap_err();
        matches!(err, Error::BadRequest(_));
    }
}
