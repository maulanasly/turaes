//! The proxy data-plane entry point.
//!
//! The routing/cert logic lives here unconditionally; only the Pingora data
//! plane is feature-gated. Enabling the `pingora` feature (Linux, M2) compiles
//! the real proxy. Without it, [`serve`] returns a clear configuration error so
//! the rest of turaes keeps working.

use std::sync::Arc;

use turaes_core::config::ProxyConfig;

use crate::router::Router;
use crate::tls::CertStore;

/// Shared proxy state handed to the data plane.
#[derive(Clone)]
pub struct ProxyState {
    /// Host → upstream routing table.
    pub router: Arc<Router>,
    /// Certificate store (certbot layout).
    pub certs: CertStore,
}

/// Build proxy state from configuration and a router.
pub fn state(proxy_cfg: &ProxyConfig, router: Router) -> ProxyState {
    ProxyState {
        router: Arc::new(router),
        certs: CertStore::new(&proxy_cfg.cert_dir),
    }
}

#[cfg(feature = "pingora")]
pub use pingora_impl::serve;

/// Report whether this build includes the Pingora data plane.
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
    use async_trait::async_trait;
    use pingora::prelude::*;
    use pingora::proxy::{ProxyHttp, Session};
    use pingora::upstreams::peer::HttpPeer;

    use turaes_core::config::ProxyConfig;
    use turaes_core::{Error, Result};

    use super::ProxyState;

    struct Gateway {
        state: ProxyState,
    }

    #[async_trait]
    impl ProxyHttp for Gateway {
        type CTX = ();
        fn new_ctx(&self) -> Self::CTX {}

        async fn upstream_peer(
            &self,
            session: &mut Session,
            _ctx: &mut Self::CTX,
        ) -> pingora::Result<Box<HttpPeer>> {
            let host = session
                .req_header()
                .headers
                .get(http::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            match self.state.router.resolve(host) {
                Some(upstream) => Ok(Box::new(HttpPeer::new(
                    upstream.addr(),
                    upstream.tls,
                    host.to_string(),
                ))),
                None => Err(pingora::Error::new(pingora::ErrorType::HTTPStatus(404))),
            }
        }
    }

    /// Run the Pingora proxy until the process exits.
    pub async fn serve(proxy_cfg: &ProxyConfig, state: ProxyState) -> Result<()> {
        let mut server = Server::new(None)
            .map_err(|e| Error::Config(format!("failed to create pingora server: {e}")))?;
        server.bootstrap();

        let gateway = Gateway { state };
        let mut service = pingora::proxy::http_proxy_service(&server.configuration, gateway);
        service.add_tcp(&format!("0.0.0.0:{}", proxy_cfg.http_port));
        server.add_service(service);
        tracing::info!(http_port = proxy_cfg.http_port, "pingora proxy listening");
        server.run_forever();
    }
}
