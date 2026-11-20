//! Lazy localhost Axum server for viewing HTML documents in an external browser.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use rand::RngCore;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const MAX_DOCUMENTS: usize = 16;

struct LruDocs {
    entries: VecDeque<(String, String)>,
}

impl LruDocs {
    fn new() -> Self {
        Self {
            entries: VecDeque::with_capacity(MAX_DOCUMENTS),
        }
    }

    fn insert(&mut self, token: String, html: String) {
        self.entries.retain(|(t, _)| t != &token);
        if self.entries.len() >= MAX_DOCUMENTS {
            self.entries.pop_front();
        }
        self.entries.push_back((token, html));
    }

    fn get(&mut self, token: &str) -> Option<String> {
        let idx = self.entries.iter().position(|(t, _)| t == token)?;
        let entry = self.entries.remove(idx)?;
        let html = entry.1.clone();
        self.entries.push_back(entry);
        Some(html)
    }
}

struct RunningServer {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

struct ServerInner {
    docs: Arc<Mutex<LruDocs>>,
    running: Mutex<Option<RunningServer>>,
    start_lock: tokio::sync::Mutex<()>,
}

impl ServerInner {
    fn shutdown_now(&self) {
        if let Ok(mut guard) = self.running.lock() {
            if let Some(mut running) = guard.take() {
                if let Some(tx) = running.shutdown_tx.take() {
                    let _ = tx.send(());
                }
            }
        }
    }
}

impl Drop for ServerInner {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

/// Thread-safe, cloneable, lazily-started localhost HTML server (`0 MB` idle).
#[derive(Clone)]
pub struct NativeHtmlServer {
    inner: Arc<ServerInner>,
}

impl NativeHtmlServer {
    /// Creates a new handle without binding any socket or spawning any task.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ServerInner {
                docs: Arc::new(Mutex::new(LruDocs::new())),
                running: Mutex::new(None),
                start_lock: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Lazily starts a localhost `axum` server (if not running) and returns `http://127.0.0.1:{port}/view/{token}`.
    pub async fn serve_html(&self, title: &str, html: &str) -> anyhow::Result<String> {
        let port = self.ensure_started().await?;
        let token = generate_token();
        let document = wrap_if_fragment(title, html);

        {
            let mut docs = self
                .inner
                .docs
                .lock()
                .map_err(|_| anyhow::anyhow!("HTML server document store poisoned"))?;
            docs.insert(token.clone(), document);
        }

        Ok(format!("http://127.0.0.1:{port}/view/{token}"))
    }

    /// Cleanly shuts down the background server, if started.
    pub fn shutdown(&self) {
        self.inner.shutdown_now();
    }

    async fn ensure_started(&self) -> anyhow::Result<u16> {
        if let Ok(guard) = self.inner.running.lock() {
            if let Some(ref running) = *guard {
                return Ok(running.port);
            }
        }

        let _init_guard = self.inner.start_lock.lock().await;
        if let Ok(guard) = self.inner.running.lock() {
            if let Some(ref running) = *guard {
                return Ok(running.port);
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let docs = Arc::clone(&self.inner.docs);

        let app = Router::new()
            .route("/view/{token}", get(handle_view))
            .with_state(docs);

        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        let mut guard = self
            .inner
            .running
            .lock()
            .map_err(|_| anyhow::anyhow!("HTML server state lock poisoned"))?;
        *guard = Some(RunningServer {
            port,
            shutdown_tx: Some(shutdown_tx),
        });

        Ok(port)
    }
}

impl Default for NativeHtmlServer {
    fn default() -> Self {
        Self::new()
    }
}

async fn handle_view(
    State(docs): State<Arc<Mutex<LruDocs>>>,
    Path(token): Path<String>,
) -> Response {
    let found = docs.lock().ok().and_then(|mut d| d.get(&token));
    match found {
        Some(html) => {
            let mut resp = (StatusCode::OK, html).into_response();
            let headers = resp.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            );
            resp
        }
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn wrap_if_fragment(title: &str, html: &str) -> String {
    let lower_head: String = html.chars().take(256).collect::<String>().to_ascii_lowercase();
    if lower_head.contains("<!doctype") || lower_head.contains("<html") {
        html.to_string()
    } else {
        let escaped_title = title
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>{escaped_title}</title></head><body>{html}</body></html>"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn http_get(host_port: &str, path: &str) -> String {
        let mut stream = TcpStream::connect(host_port).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = String::new();
        stream.read_to_string(&mut buf).await.unwrap();
        buf
    }

    #[tokio::test]
    async fn serves_html_and_returns_404_for_unknown_token() {
        let server = NativeHtmlServer::new();
        let url = server
            .serve_html("Test Doc", "<h1>Hello Axum</h1>")
            .await
            .unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));

        let without_scheme = url.strip_prefix("http://").unwrap();
        let (host_port, path) = without_scheme.split_once('/').unwrap();
        let path = format!("/{path}");

        let ok_resp = http_get(host_port, &path).await;
        assert!(ok_resp.starts_with("HTTP/1.1 200 OK"));
        assert!(ok_resp
            .to_ascii_lowercase()
            .contains("content-type: text/html; charset=utf-8"));
        assert!(ok_resp
            .to_ascii_lowercase()
            .contains("x-content-type-options: nosniff"));
        assert!(ok_resp
            .to_ascii_lowercase()
            .contains("cache-control: no-store"));
        assert!(ok_resp.contains("<h1>Hello Axum</h1>"));

        let missing_resp = http_get(host_port, "/view/00000000000000000000000000000000").await;
        assert!(missing_resp.starts_with("HTTP/1.1 404"));

        server.shutdown();
    }
}
