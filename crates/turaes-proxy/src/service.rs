//! The proxy data-plane entry point.
//!
//! Host routing and certificate discovery are always compiled; only the Pingora
//! data plane is behind the `pingora` feature. TLS certificates are issued by
//! certbot and loaded from disk — turaes never speaks ACME itself.
//!
//! Pingora's `run_forever()` blocks and drives its own async runtime, so it must
//! run on a **dedicated OS thread** — never inside the turaes Tokio runtime
//! (otherwise: "Cannot start a runtime from within a runtime").

use std::sync::Arc;

use turaes_core::config::ProxyConfig;

use crate::router::Router;
use crate::tls::CertStore;

/// Shared proxy state handed to the data plane.
#[derive(Clone)]
pub struct ProxyState {
    /// Host → upstream routing table (hot-swappable).
    pub router: Arc<Router>,
    /// Certificate store (certbot layout).
    pub certs: CertStore,
    /// Hostname of the turaes dashboard (TLS cert + default route).
    pub dashboard_host: Option<String>,
}

/// Build proxy state from configuration and a router.
pub fn state(
    proxy_cfg: &ProxyConfig,
    router: Arc<Router>,
    dashboard_host: Option<String>,
) -> ProxyState {
    ProxyState {
        router,
        certs: CertStore::new(&proxy_cfg.cert_dir),
        dashboard_host,
    }
}

/// Whether this build includes the Pingora data plane.
pub fn pingora_enabled() -> bool {
    cfg!(feature = "pingora")
}

/// Extract a certbot HTTP-01 challenge token from a request path, if valid.
pub fn acme_token(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/.well-known/acme-challenge/")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    if rest
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(rest)
    } else {
        None
    }
}

/// Start the proxy on a background OS thread. Non-blocking.
#[cfg(feature = "pingora")]
pub fn spawn(proxy_cfg: ProxyConfig, state: ProxyState) {
    let handle = std::thread::Builder::new()
        .name("pingora-proxy".into())
        .spawn(move || {
            if let Err(e) = pingora_impl::serve(&proxy_cfg, state) {
                tracing::error!(error = %e, "proxy exited");
            }
        });
    if let Err(e) = handle {
        tracing::error!(error = %e, "failed to start proxy thread");
    }
}

/// No-op when the `pingora` feature is disabled.
#[cfg(not(feature = "pingora"))]
pub fn spawn(_proxy_cfg: ProxyConfig, _state: ProxyState) {
    tracing::warn!(
        "proxy.enabled is set but this build lacks the `pingora` feature; rebuild with `--features proxy`"
    );
}

#[cfg(feature = "pingora")]
mod pingora_impl {
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use async_trait::async_trait;
    use pingora::listeners::tls::TlsSettings;
    use pingora::listeners::{TcpSocketOptions, TlsAccept};
    use pingora::prelude::*;
    use pingora::protocols::tls::TlsRef;
    use pingora::tls::pkey::{PKey, Private};
    use pingora::tls::ssl::NameType;
    use pingora::tls::x509::X509;

    use turaes_core::config::ProxyConfig;

    use crate::router::Router;
    use crate::tls::CertStore;

    use super::ProxyState;

    /// A loaded certificate + key tagged with the source file's mtime so it is
    /// reloaded when the file changes (cert distributed/renewed on disk).
    struct Entry {
        cert: X509,
        key: PKey<Private>,
        mtime: Option<SystemTime>,
    }

    fn load_entry(fullchain: &Path, key: &Path) -> Option<Entry> {
        let cert = X509::from_pem(&std::fs::read(fullchain).ok()?).ok()?;
        let pkey = PKey::private_key_from_pem(&std::fs::read(key).ok()?).ok()?;
        let mtime = std::fs::metadata(fullchain).and_then(|m| m.modified()).ok();
        Some(Entry {
            cert,
            key: pkey,
            mtime,
        })
    }

    fn use_entry(ssl: &mut TlsRef, entry: &Entry) -> bool {
        use pingora::tls::ext;
        ext::ssl_use_certificate(ssl, &entry.cert).is_ok()
            && ext::ssl_use_private_key(ssl, &entry.key).is_ok()
    }

    /// Selects a certificate per SNI from the certbot layout, falling back to
    /// the dashboard certificate. Certs reload automatically when their files
    /// change on disk.
    struct SniCertStore {
        certs: CertStore,
        default_host: String,
        default: Mutex<Entry>,
        cache: Mutex<HashMap<String, Entry>>,
    }

    impl SniCertStore {
        fn new(certs: CertStore, default_host: &str) -> Result<Self, String> {
            let paths = certs.paths(default_host);
            let default = load_entry(&paths.fullchain, &paths.private_key)
                .ok_or_else(|| format!("missing certificate for {default_host}"))?;
            Ok(Self {
                certs,
                default_host: default_host.to_string(),
                default: Mutex::new(default),
                cache: Mutex::new(HashMap::new()),
            })
        }
    }

    /// Reload a cached cert if its file mtime changed.
    fn refresh(certs: &CertStore, cache: &mut HashMap<String, Entry>, name: &str) {
        let paths = certs.paths(name);
        let mtime = std::fs::metadata(&paths.fullchain)
            .and_then(|m| m.modified())
            .ok();
        let stale = cache.get(name).map(|e| e.mtime != mtime).unwrap_or(true);
        if stale {
            match load_entry(&paths.fullchain, &paths.private_key) {
                Some(entry) => {
                    cache.insert(name.to_string(), entry);
                }
                None => {
                    cache.remove(name);
                }
            }
        }
    }

    #[async_trait]
    impl TlsAccept for SniCertStore {
        async fn certificate_callback(&self, ssl: &mut TlsRef) {
            if let Some(name) = ssl.servername(NameType::HOST_NAME) {
                let name = name.to_ascii_lowercase();
                let mut cache = self.cache.lock().unwrap();
                refresh(&self.certs, &mut cache, &name);
                if let Some(entry) = cache.get(&name) {
                    if use_entry(ssl, entry) {
                        return;
                    }
                }
            }

            // Fall back to the default (dashboard) certificate, reloading on change.
            let mut default = self.default.lock().unwrap();
            let paths = self.certs.paths(&self.default_host);
            let mtime = std::fs::metadata(&paths.fullchain)
                .and_then(|m| m.modified())
                .ok();
            if default.mtime != mtime {
                if let Some(entry) = load_entry(&paths.fullchain, &paths.private_key) {
                    *default = entry;
                }
            }
            let _ = use_entry(ssl, &default);
        }
    }

    struct Gateway {
        router: Arc<Router>,
        acme_webroot: String,
        counter: std::sync::atomic::AtomicUsize,
    }

    impl Gateway {
        /// Serve the parked-host maintenance page (503). Known hostnames
        /// without live upstreams (stopped or unhealthy apps) get branded
        /// HTML naming the application — never a bare 502.
        async fn serve_maintenance(
            &self,
            session: &mut Session,
            parked: &crate::router::ParkedHost,
        ) -> Result<bool> {
            let body = crate::maintenance::body(&parked.app, parked.reason);
            let mut resp = ResponseHeader::build(503, None)?;
            resp.insert_header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")?;
            resp.insert_header(http::header::RETRY_AFTER, "60")?;
            session.write_response_header(Box::new(resp), false).await?;
            session
                .write_response_body(Some(bytes::Bytes::from(body)), true)
                .await?;
            Ok(true)
        }

        /// Serve a certbot HTTP-01 challenge file from the webroot
        /// (`{webroot}/.well-known/acme-challenge/{token}`).
        async fn serve_acme(&self, session: &mut Session, token: &str) -> Result<bool> {
            let file = Path::new(&self.acme_webroot)
                .join(".well-known")
                .join("acme-challenge")
                .join(token);
            match std::fs::read(&file) {
                Ok(body) => {
                    let mut resp = ResponseHeader::build(200, None)?;
                    resp.insert_header(http::header::CONTENT_TYPE, "text/plain")?;
                    session.write_response_header(Box::new(resp), false).await?;
                    session
                        .write_response_body(Some(bytes::Bytes::from(body)), true)
                        .await?;
                }
                Err(_) => {
                    let resp = ResponseHeader::build(404, None)?;
                    session.write_response_header(Box::new(resp), true).await?;
                }
            }
            Ok(true)
        }
    }

    #[async_trait]
    impl ProxyHttp for Gateway {
        type CTX = ();
        fn new_ctx(&self) -> Self::CTX {}

        /// Redirect plain HTTP to HTTPS for known hosts; serve the parked-host
        /// maintenance page (503) for hostnames without live upstreams.
        /// Unknown hosts (and IP access) fall through so they 404 as before.
        async fn request_filter(
            &self,
            session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<bool> {
            let is_tls = session
                .digest()
                .map(|d| d.ssl_digest.is_some())
                .unwrap_or(false);

            let (host, path) = {
                let req = session.req_header();
                // HTTP/1.1 carries `Host`; HTTP/2 exposes the host via the URI
                // authority (`:authority`) instead — same fallback as
                // `upstream_peer`, or parked hosts are invisible over H2.
                let host = req
                    .headers
                    .get(http::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .or_else(|| req.uri.host())
                    .unwrap_or_default()
                    .to_string();
                let path = req
                    .uri
                    .path_and_query()
                    .map(|p| p.as_str().to_string())
                    .unwrap_or_else(|| "/".to_string());
                (host, path)
            };
            let hostname = host.split(':').next().unwrap_or("").to_ascii_lowercase();

            // certbot HTTP-01 challenge: serve from the webroot before
            // anything else, so renewals work even for parked hosts.
            // (Issuers only ever validate over plain HTTP.)
            if !is_tls {
                if let Some(token) = super::acme_token(&path) {
                    return self.serve_acme(session, token).await;
                }
            }

            // Parked hosts (stopped or unhealthy apps) get the maintenance
            // page on both schemes — no redirect to a dead endpoint.
            if let Some(parked) = self.router.is_parked(&hostname) {
                return self.serve_maintenance(session, &parked).await;
            }

            if is_tls {
                return Ok(false);
            }

            if hostname.is_empty() || self.router.resolve(&hostname).is_none() {
                return Ok(false);
            }

            let location = format!("https://{hostname}{path}");
            let mut resp = ResponseHeader::build(http::StatusCode::MOVED_PERMANENTLY, None)?;
            resp.insert_header(http::header::LOCATION, location)?;
            // Empty body; `end_of_stream = true` completes the response.
            session.write_response_header(Box::new(resp), true).await?;
            Ok(true)
        }

        async fn upstream_peer(
            &self,
            session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<Box<HttpPeer>> {
            // HTTP/1.1 carries `Host`; HTTP/2 exposes the host via the URI
            // authority (`:authority`). Pingora guarantees one of the two is
            // present for a valid request.
            let req = session.req_header();
            let host = req
                .headers
                .get(http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .or_else(|| req.uri.host())
                .unwrap_or_default()
                .to_string();
            // Round-robin across an app's replicas (one upstream => always it).
            let upstreams = self.router.resolve_all(&host);
            let n = self
                .counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u64;
            match crate::router::pick(&upstreams, n) {
                Some(upstream) => Ok(Box::new(HttpPeer::new(
                    upstream.addr(),
                    false,
                    String::new(),
                ))),
                None => Err(pingora::Error::new(pingora::ErrorType::HTTPStatus(404))),
            }
        }
    }

    /// Build and run the Pingora server. Blocks until the process exits.
    pub fn serve(proxy_cfg: &ProxyConfig, state: ProxyState) -> turaes_core::Result<()> {
        let mut server = Server::new(None).map_err(|e| {
            turaes_core::Error::Config(format!("failed to create pingora server: {e}"))
        })?;
        server.bootstrap();

        let gateway = Gateway {
            router: state.router.clone(),
            acme_webroot: proxy_cfg.acme_webroot.clone(),
            counter: std::sync::atomic::AtomicUsize::new(0),
        };
        let mut service = http_proxy_service(&server.configuration, gateway);
        // SO_REUSEPORT lets a rolling replace bind the same ports alongside the
        // old edge instance (zero-downtime edge upgrades).
        let reuse = proxy_cfg.reuse_port;
        let mut http_opts = TcpSocketOptions::default();
        http_opts.so_reuseport = reuse.then_some(true);
        service.add_tcp_with_settings(&format!("0.0.0.0:{}", proxy_cfg.http_port), http_opts);

        if let Some(host) = state.dashboard_host.as_deref() {
            match SniCertStore::new(state.certs.clone(), host) {
                Ok(store) => match TlsSettings::with_callbacks(Box::new(store)) {
                    Ok(mut tls) => {
                        tls.enable_h2();
                        let mut tls_opts = TcpSocketOptions::default();
                        tls_opts.so_reuseport = reuse.then_some(true);
                        service.add_tls_with_settings(
                            &format!("0.0.0.0:{}", proxy_cfg.https_port),
                            Some(tls_opts),
                            tls,
                        );
                        tracing::info!(
                            host,
                            https_port = proxy_cfg.https_port,
                            "proxy TLS enabled (multi-cert SNI)"
                        );
                    }
                    Err(e) => tracing::error!(error = %e, "failed to build TLS settings"),
                },
                Err(e) => tracing::warn!(host, error = %e, "no dashboard cert; HTTPS disabled"),
            }
        }

        server.add_service(service);
        tracing::info!(http_port = proxy_cfg.http_port, "pingora proxy listening");
        server.run_forever();
    }
}

#[cfg(test)]
mod tests {
    use super::{acme_token, pingora_enabled};

    #[test]
    fn acme_token_accepts_valid_and_rejects_bad() {
        assert_eq!(
            acme_token("/.well-known/acme-challenge/abc-123_XYZ"),
            Some("abc-123_XYZ")
        );
        assert_eq!(acme_token("/.well-known/acme-challenge/"), None);
        assert_eq!(acme_token("/.well-known/acme-challenge/a/b"), None);
        assert_eq!(
            acme_token("/.well-known/acme-challenge/../etc/passwd"),
            None
        );
        assert_eq!(acme_token("/health"), None);
        // sanity: default build has no data plane
        let _ = pingora_enabled();
    }
}
