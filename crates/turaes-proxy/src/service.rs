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
    use std::sync::Arc;

    use async_trait::async_trait;
    use pingora::listeners::tls::TlsSettings;
    use pingora::prelude::*;

    use turaes_core::config::ProxyConfig;

    use crate::router::Router;

    use super::ProxyState;

    struct Gateway {
        router: Arc<Router>,
    }

    #[async_trait]
    impl ProxyHttp for Gateway {
        type CTX = ();
        fn new_ctx(&self) -> Self::CTX {}

        /// Redirect plain HTTP to HTTPS for any host we route. Unknown hosts
        /// (and IP access) fall through so they 404 as before.
        async fn request_filter(
            &self,
            session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<bool> {
            let is_tls = session
                .digest()
                .map(|d| d.ssl_digest.is_some())
                .unwrap_or(false);
            if is_tls {
                return Ok(false);
            }

            let (host, path) = {
                let req = session.req_header();
                let host = req
                    .headers
                    .get(http::header::HOST)
                    .and_then(|v| v.to_str().ok())
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
            match self.router.resolve(&host) {
                // Loopback upstreams speak plain HTTP.
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
        };
        let mut service = http_proxy_service(&server.configuration, gateway);
        service.add_tcp(&format!("0.0.0.0:{}", proxy_cfg.http_port));

        if let Some(host) = state.dashboard_host.as_deref() {
            let paths = state.certs.paths(host);
            if paths.fullchain.is_file() && paths.private_key.is_file() {
                match TlsSettings::intermediate(
                    &paths.fullchain.to_string_lossy(),
                    &paths.private_key.to_string_lossy(),
                ) {
                    Ok(mut tls) => {
                        tls.enable_h2();
                        service.add_tls_with_settings(
                            &format!("0.0.0.0:{}", proxy_cfg.https_port),
                            None,
                            tls,
                        );
                        tracing::info!(
                            host,
                            https_port = proxy_cfg.https_port,
                            "proxy TLS enabled"
                        );
                    }
                    Err(e) => tracing::error!(error = %e, "failed to load TLS settings"),
                }
            } else {
                tracing::warn!(
                    host,
                    "no certificate found for dashboard host; HTTPS disabled"
                );
            }
        }

        server.add_service(service);
        tracing::info!(http_port = proxy_cfg.http_port, "pingora proxy listening");
        server.run_forever();
    }
}
