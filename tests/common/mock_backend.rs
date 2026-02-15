//! Mock backend server for testing proxy functionality.

#![allow(dead_code)]

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::routing::any;
use axum::Router;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// A captured request for assertions.
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A mock response to return.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay_ms: u64,
}

impl Default for MockResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: br#"{"ok": true}"#.to_vec(),
            delay_ms: 0,
        }
    }
}

impl MockResponse {
    pub fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: body.as_bytes().to_vec(),
            delay_ms: 0,
        }
    }

    pub fn error(status: u16, message: &str) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: format!(r#"{{"error": "{}"}}"#, message).into_bytes(),
            delay_ms: 0,
        }
    }

    pub fn sse(events: &[&str]) -> Self {
        let body: String = events
            .iter()
            .map(|e| {
                // Extract event type from JSON for realistic Anthropic SSE format.
                // Real streams include `event: <type>\n` before `data: {...}\n\n`.
                let event_type = serde_json::from_str::<serde_json::Value>(e)
                    .ok()
                    .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from));
                match event_type {
                    Some(t) => format!("event: {}\ndata: {}\n\n", t, e),
                    None => format!("data: {}\n\n", e),
                }
            })
            .collect();
        Self {
            status: 200,
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
            body: body.into_bytes(),
            delay_ms: 0,
        }
    }

    pub fn sse_with_status(status: u16, events: &[&str]) -> Self {
        let mut resp = Self::sse(events);
        resp.status = status;
        resp
    }

    pub fn with_delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }
}

#[derive(Clone)]
struct MockState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
}

/// Mock backend server for testing.
pub struct MockBackend {
    pub addr: SocketAddr,
    state: MockState,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl MockBackend {
    /// Start a new mock backend server.
    pub async fn start() -> Self {
        let state = MockState {
            requests: Arc::new(Mutex::new(Vec::new())),
            responses: Arc::new(Mutex::new(VecDeque::new())),
        };

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let app = Router::new()
            .route("/{*path}", any(handle_request))
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind mock server");
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.changed().await;
                })
                .await
                .ok();
        });

        // Wait for server to be ready
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        Self {
            addr,
            state,
            shutdown: shutdown_tx,
        }
    }

    /// Enqueue a response to be returned for the next request.
    pub async fn enqueue_response(&self, resp: MockResponse) {
        self.state.responses.lock().await.push_back(resp);
    }

    /// Get all captured requests.
    pub async fn captured_requests(&self) -> Vec<CapturedRequest> {
        self.state.requests.lock().await.clone()
    }

    /// Get the base URL for this mock server.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Clear captured requests.
    pub async fn clear(&self) {
        self.state.requests.lock().await.clear();
        self.state.responses.lock().await.clear();
    }
}

impl Drop for MockBackend {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn handle_request(
    State(state): State<MockState>,
    req: Request<Body>,
) -> Response<Body> {
    // Capture request
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let body_bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default()
        .to_vec();

    state.requests.lock().await.push(CapturedRequest {
        method,
        path,
        headers,
        body: body_bytes,
    });

    // Get next response or return default
    let mock_resp = state
        .responses
        .lock()
        .await
        .pop_front()
        .unwrap_or_default();

    // Apply delay if configured
    if mock_resp.delay_ms > 0 {
        tokio::time::sleep(tokio::time::Duration::from_millis(mock_resp.delay_ms)).await;
    }

    // Build response
    let mut builder = Response::builder().status(StatusCode::from_u16(mock_resp.status).unwrap());

    for (name, value) in mock_resp.headers {
        builder = builder.header(name, value);
    }

    builder.body(Body::from(mock_resp.body)).unwrap()
}
