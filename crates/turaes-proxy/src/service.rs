//! The proxy data-plane entry point.
//!
//! Host routing and certificate discovery are always compiled; only the Pingora
//! data plane is behind the `pingora` feature. TLS certificates are issued by
//! certbot and loaded from disk — turaes never speaks ACME itself.

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

#[cfg(feature = "pingora")]
pub use pingora_impl::serve;

/// Whether this build includes the Pingora data plane.
pub fn pingora_enabled() -> bool {
    cfg!(feature = "pingora")
}

#[cfg(not(feature = "pingora"))]
/// No-op when the `pingora` feature is disabled.
pub async fn serve(_proxy_cfg: &ProxyConfig, _state: ProxyState) -> turaes_core::Result<()> {
    Err(turaes_core::Error::Config(
        "turaes-proxy was built without the `pingora` feature; rebuild with --features pingora"
            .into(),
    ))
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

        async fn upstream_peer(
            &self,
            session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> Result<Box<HttpPeer>> {
            let host = session
                .req_header()
                .headers
                .get(http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            match self.router.resolve(host) {
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

    /// Run the Pingora proxy until the process exits.
    pub async fn serve(proxy_cfg: &ProxyConfig, state: ProxyState) -> turaes_core::Result<()> {
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
