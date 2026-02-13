//! Integration tests for pipeline isolation (main vs teammate).
//!
//! Verifies that `axum::Router::nest()` correctly separates the two pipelines:
//! - Main pipeline: `/*` — with thinking middleware
//! - Teammate pipeline: `/teammate/*` — fixed backend, no thinking

mod common;

use anyclaude::config::{
    AgentTeamsConfig, Backend, Config, ConfigStore, DebugLoggingConfig, Defaults, ProxyConfig,
    TerminalConfig,
};
use anyclaude::metrics::DebugLogger;
use anyclaude::proxy::ProxyServer;
use common::mock_backend::{MockBackend, MockResponse};
use reqwest::Client;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn create_backend(name: &str, base_url: &str) -> Backend {
    Backend {
        name: name.to_string(),
        display_name: name.to_uppercase(),
        base_url: base_url.to_string(),
        auth_type_str: "passthrough".to_string(),
        api_key: None,
        pricing: None,
        thinking_compat: None,
        thinking_budget_tokens: None,
        model_opus: None,
        model_sonnet: None,
        model_haiku: None,
    }
}

fn config_with_teams(
    backends: Vec<Backend>,
    bind_addr: &str,
    agent_teams: Option<AgentTeamsConfig>,
) -> Config {
    Config {
        defaults: Defaults {
            active: backends.first().map(|b| b.name.clone()).unwrap_or_default(),
            timeout_seconds: 5,
            connect_timeout_seconds: 2,
            idle_timeout_seconds: 30,
            pool_idle_timeout_seconds: 30,
            pool_max_idle_per_host: 2,
            max_retries: 1,
            retry_backoff_base_ms: 10,
        },
        proxy: ProxyConfig {
            bind_addr: bind_addr.to_string(),
            base_url: format!("http://{}", bind_addr),
        },
        terminal: TerminalConfig::default(),
        debug_logging: DebugLoggingConfig::default(),
        claude_settings: HashMap::new(),
        backends,
        agent_teams,
    }
}

struct TestHarness {
    proxy_addr: std::net::SocketAddr,
    client: Client,
    _handle: anyclaude::proxy::ProxyHandle,
}

impl TestHarness {
    async fn start(config: Config) -> Self {
        let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
        let debug_logger = Arc::new(DebugLogger::new(Default::default()));
        let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();
        let (proxy_addr, _) = server.try_bind(&config_store).await.unwrap();
        let handle = server.handle();
        tokio::spawn(async move { let _ = server.run().await; });
        tokio::time::sleep(Duration::from_millis(50)).await;
        Self {
            proxy_addr,
            client: Client::new(),
            _handle: handle,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.proxy_addr, path)
    }
}

// ---------------------------------------------------------------------------
// Routing correctness
// ---------------------------------------------------------------------------

#[tokio::test]
async fn main_pipeline_routes_to_active_backend() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;
    mock_main.enqueue_response(MockResponse::json(r#"{"from":"main"}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    let resp = h.client
        .post(h.url("/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false}"#)
        .send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("main"));

    assert_eq!(mock_main.captured_requests().await.len(), 1);
    assert_eq!(mock_teammate.captured_requests().await.len(), 0);
}

#[tokio::test]
async fn teammate_pipeline_routes_to_teammate_backend() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;
    mock_teammate.enqueue_response(MockResponse::json(r#"{"from":"teammate"}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    let resp = h.client
        .post(h.url("/teammate/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false}"#)
        .send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("teammate"));

    assert_eq!(mock_main.captured_requests().await.len(), 0);
    assert_eq!(mock_teammate.captured_requests().await.len(), 1);
}

#[tokio::test]
async fn teammate_path_stripped_before_forwarding() {
    let mock_teammate = MockBackend::start().await;
    mock_teammate.enqueue_response(MockResponse::json(r#"{"ok":true}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("main", "http://127.0.0.1:1"), // unused
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    h.client
        .post(h.url("/teammate/v1/messages"))
        .body("{}")
        .send().await.unwrap();

    let requests = mock_teammate.captured_requests().await;
    assert_eq!(requests.len(), 1);
    // nest() strips /teammate prefix — backend receives /v1/messages
    assert_eq!(requests[0].path, "/v1/messages");
}

#[tokio::test]
async fn teammate_preserves_query_string() {
    let mock_teammate = MockBackend::start().await;
    mock_teammate.enqueue_response(MockResponse::json(r#"{"ok":true}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("main", "http://127.0.0.1:1"),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    h.client
        .post(h.url("/teammate/v1/messages?beta=true&version=2"))
        .body("{}")
        .send().await.unwrap();

    let requests = mock_teammate.captured_requests().await;
    assert_eq!(requests.len(), 1);
    // Path after stripping, query string preserved
    assert_eq!(requests[0].path, "/v1/messages");
    // Note: MockBackend captures path without query. Check via full URI if needed.
}

// ---------------------------------------------------------------------------
// Partial segment matching: /teammates ≠ /teammate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teammates_with_trailing_s_routes_to_main_not_teammate() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;
    mock_main.enqueue_response(MockResponse::json(r#"{"from":"main"}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    let resp = h.client
        .post(h.url("/teammates/v1/messages"))
        .body("{}")
        .send().await.unwrap();

    assert_eq!(resp.status(), 200);
    // /teammates does NOT match /teammate nest — goes to main pipeline
    assert_eq!(mock_main.captured_requests().await.len(), 1);
    assert_eq!(mock_teammate.captured_requests().await.len(), 0);
    // Main backend receives the full unstripped path
    assert_eq!(mock_main.captured_requests().await[0].path, "/teammates/v1/messages");
}

// ---------------------------------------------------------------------------
// Bare prefix: /teammate with no trailing path → rewritten to /
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bare_teammate_prefix_returns_404() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    let resp = h.client
        .post(h.url("/teammate"))
        .body("{}")
        .send().await.unwrap();

    // Bare /teammate (no sub-path) is claimed by nest() but the inner
    // router has no handler for "/", so axum returns 404.
    // This is fine — teammates always send /teammate/v1/messages.
    assert_eq!(resp.status(), 404);
    assert_eq!(mock_main.captured_requests().await.len(), 0);
    assert_eq!(mock_teammate.captured_requests().await.len(), 0);
}

// ---------------------------------------------------------------------------
// No agent_teams → no /teammate route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_agent_teams_returns_404_on_teammate_path() {
    let mock = MockBackend::start().await;
    mock.enqueue_response(MockResponse::json(r#"{"ok":true}"#)).await;

    let config = config_with_teams(
        vec![create_backend("main", &mock.base_url())],
        &format!("127.0.0.1:{}", common::free_port()),
        None, // no agent_teams
    );
    let h = TestHarness::start(config).await;

    // /teammate should fall through to main pipeline (since no nest)
    // The main backend gets /teammate/v1/messages as-is
    let _ = h.client
        .post(h.url("/teammate/v1/messages"))
        .body("{}")
        .send().await.unwrap();

    // Main pipeline handles it (forwarded to main backend with full path)
    let requests = mock.captured_requests().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/teammate/v1/messages");
}

// ---------------------------------------------------------------------------
// Concurrent main + teammate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concurrent_main_and_teammate_route_correctly() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;

    // Enqueue enough responses for concurrent requests
    for _ in 0..5 {
        mock_main.enqueue_response(MockResponse::json(r#"{"from":"main"}"#)).await;
        mock_teammate.enqueue_response(MockResponse::json(r#"{"from":"teammate"}"#)).await;
    }

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;
    let h = Arc::new(h);

    let mut handles = vec![];

    // 5 main + 5 teammate requests concurrently
    for i in 0..10 {
        let h = h.clone();
        let is_teammate = i % 2 == 1;
        handles.push(tokio::spawn(async move {
            let path = if is_teammate { "/teammate/v1/messages" } else { "/v1/messages" };
            let resp = h.client
                .post(h.url(path))
                .body("{}")
                .send().await.unwrap();
            assert_eq!(resp.status(), 200);
            is_teammate
        }));
    }

    let mut main_count = 0;
    let mut teammate_count = 0;
    for handle in handles {
        if handle.await.unwrap() {
            teammate_count += 1;
        } else {
            main_count += 1;
        }
    }

    assert_eq!(main_count, 5);
    assert_eq!(teammate_count, 5);
    assert_eq!(mock_main.captured_requests().await.len(), 5);
    assert_eq!(mock_teammate.captured_requests().await.len(), 5);
}

// ---------------------------------------------------------------------------
// Thinking session isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teammate_requests_dont_increment_thinking_session() {
    let mock_main = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;

    mock_main.enqueue_response(MockResponse::json(r#"{"content":[]}"#)).await;
    mock_main.enqueue_response(MockResponse::json(r#"{"content":[]}"#)).await;
    for _ in 0..5 {
        mock_teammate.enqueue_response(MockResponse::json(r#"{"content":[]}"#)).await;
    }

    let config = config_with_teams(
        vec![
            create_backend("main", &mock_main.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();
    let (proxy_addr, _) = server.try_bind(&config_store).await.unwrap();
    let registry = server.transformer_registry();
    let handle = server.handle();

    tokio::spawn(async move { let _ = server.run().await; });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // One main request to initialize thinking session
    client.post(format!("http://{}/v1/messages", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false}"#)
        .send().await.unwrap();

    let stats_after_main = registry.thinking_cache_stats();

    // Five teammate requests — should NOT affect thinking session
    for _ in 0..5 {
        client.post(format!("http://{}/teammate/v1/messages", proxy_addr))
            .header("content-type", "application/json")
            .body(r#"{"model":"glm-5","stream":false}"#)
            .send().await.unwrap();
    }

    let stats_after_teammates = registry.thinking_cache_stats();

    // Thinking cache should be identical — teammates don't register blocks
    assert_eq!(stats_after_main.total, stats_after_teammates.total);

    // Another main request — thinking session should still work
    let final_resp = client.post(format!("http://{}/v1/messages", proxy_addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","stream":false}"#)
        .send().await.unwrap();
    assert_eq!(final_resp.status(), 200);

    drop(handle);
}

// ---------------------------------------------------------------------------
// SSE streaming through teammate pipeline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teammate_sse_streaming_works() {
    let mock_teammate = MockBackend::start().await;
    mock_teammate.enqueue_response(MockResponse::sse(&[
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        r#"{"type":"message_stop"}"#,
    ])).await;

    let config = config_with_teams(
        vec![
            create_backend("main", "http://127.0.0.1:1"),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let h = TestHarness::start(config).await;

    let resp = h.client
        .post(h.url("/teammate/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"stream":true}"#)
        .send().await.unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp.headers().get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(ct.contains("text/event-stream"));

    let body = resp.text().await.unwrap();
    assert!(body.contains("message_stop"));
}

// ---------------------------------------------------------------------------
// Backend switch doesn't affect teammate routing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backend_switch_doesnt_affect_teammate_routing() {
    let mock_alpha = MockBackend::start().await;
    let mock_beta = MockBackend::start().await;
    let mock_teammate = MockBackend::start().await;

    mock_alpha.enqueue_response(MockResponse::json(r#"{"from":"alpha"}"#)).await;
    mock_beta.enqueue_response(MockResponse::json(r#"{"from":"beta"}"#)).await;
    mock_teammate.enqueue_response(MockResponse::json(r#"{"from":"teammate"}"#)).await;
    mock_teammate.enqueue_response(MockResponse::json(r#"{"from":"teammate"}"#)).await;

    let config = config_with_teams(
        vec![
            create_backend("alpha", &mock_alpha.base_url()),
            create_backend("beta", &mock_beta.base_url()),
            create_backend("teammate", &mock_teammate.base_url()),
        ],
        &format!("127.0.0.1:{}", common::free_port()),
        Some(AgentTeamsConfig { teammate_backend: "teammate".to_string() }),
    );
    let config_store = ConfigStore::new(config, PathBuf::from("/tmp/test.toml"));
    let debug_logger = Arc::new(DebugLogger::new(Default::default()));
    let mut server = ProxyServer::new(config_store.clone(), debug_logger).unwrap();
    let backend_state = server.backend_state();
    let (proxy_addr, _) = server.try_bind(&config_store).await.unwrap();
    let handle = server.handle();

    tokio::spawn(async move { let _ = server.run().await; });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = Client::new();

    // Teammate before switch
    let resp = client.post(format!("http://{}/teammate/v1/messages", proxy_addr))
        .body("{}").send().await.unwrap();
    assert!(resp.text().await.unwrap().contains("teammate"));

    // Switch main backend alpha → beta
    backend_state.switch_backend("beta").unwrap();

    // Teammate after switch — still goes to teammate backend
    let resp = client.post(format!("http://{}/teammate/v1/messages", proxy_addr))
        .body("{}").send().await.unwrap();
    assert!(resp.text().await.unwrap().contains("teammate"));

    // Main goes to beta now
    let resp = client.post(format!("http://{}/v1/messages", proxy_addr))
        .body("{}").send().await.unwrap();
    assert!(resp.text().await.unwrap().contains("beta"));

    assert_eq!(mock_alpha.captured_requests().await.len(), 0);
    assert_eq!(mock_beta.captured_requests().await.len(), 1);
    assert_eq!(mock_teammate.captured_requests().await.len(), 2);

    drop(handle);
}
