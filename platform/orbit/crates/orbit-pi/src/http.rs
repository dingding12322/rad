//! Remote-image transport for GPUI.
//!
//! Orbit is local-first; the only network traffic is author avatars requested
//! by `img("https://…")`. GPUI's image loader needs an [`HttpClient`], and its
//! default is a null client that renders nothing — so this module supplies a
//! small blocking client whose requests run on a worker thread, keeping the
//! GPUI executor free. TLS uses rustls with the platform trust store and
//! Mozilla's webpki roots as fallback.

use anyhow::{anyhow, Result};
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::FutureExt;
use gpui::http_client::http::HeaderValue;
use gpui::http_client::{AsyncBody, HttpClient, Inner, Request, Response, Url};
use std::sync::Arc;

const USER_AGENT: &str = concat!("Orbit/", env!("CARGO_PKG_VERSION"));

/// The app's HTTP client, or `None` if the TLS stack can't start — in which
/// case remote images fall back to their monograms.
pub fn avatar_client() -> Option<Arc<dyn HttpClient>> {
    let client = reqwest::blocking::Client::builder().build().ok()?;
    Some(Arc::new(AvatarClient { client }))
}

struct AvatarClient {
    client: reqwest::blocking::Client,
}

impl HttpClient for AvatarClient {
    fn type_name(&self) -> &'static str {
        "AvatarClient"
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, request: Request<AsyncBody>) -> BoxFuture<'static, Result<Response<AsyncBody>>> {
        let (parts, body) = request.into_parts();
        // Only GPUI's image GETs reach this client; reject streaming bodies
        // rather than pretend to support them.
        let payload = match body.0 {
            Inner::Empty => Vec::new(),
            Inner::Bytes(cursor) => cursor.into_inner().to_vec(),
            Inner::AsyncReader(_) => {
                return async { Err(anyhow!("streaming request bodies are not supported")) }
                    .boxed();
            }
        };
        let method = parts.method.as_str().to_owned();
        let uri = parts.uri.to_string();

        let client = self.client.clone();
        let (tx, rx) = oneshot::channel();
        // Blocking I/O off the GPUI executor; one short-lived worker per
        // request. GPUI caches decoded images per URI, so this runs rarely.
        std::thread::spawn(move || {
            let result = client
                .request(
                    reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
                    &uri,
                )
                .header("User-Agent", USER_AGENT)
                .body(payload)
                .send()
                .map_err(|err| anyhow!("{err}"))
                .and_then(into_gpui_response);
            let _ = tx.send(result);
        });

        async move {
            rx.await
                .map_err(|_| anyhow!("http worker dropped the request"))?
        }
        .boxed()
    }
}

/// Convert a reqwest response into the shape GPUI's image loader reads.
fn into_gpui_response(response: reqwest::blocking::Response) -> Result<Response<AsyncBody>> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = response.bytes().map_err(|err| anyhow!("{err}"))?;
    let mut builder = Response::builder().status(status);
    for (name, value) in headers.iter() {
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder
        .body(AsyncBody::from(bytes.to_vec()))
        .map_err(Into::into)
}
