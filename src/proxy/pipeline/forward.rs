//! Stage 6: Forward request with retry.
//!
//! Sends the request to the upstream backend with retry logic for
//! connection errors and timeouts.

use axum::http::{Method, Uri};
use reqwest::Client;
use tokio::time::sleep;

use crate::config::Backend;
use crate::proxy::error::ProxyError;
use crate::proxy::pipeline::{PipelineConfig, PipelineContext};

/// Stage 6: Forward request to upstream with retry logic.
///
/// Returns the raw upstream response for Stage 7 to handle.
pub async fn forward_with_retry(
    client: &Client,
    method: Method,
    uri: Uri,
    headers: Vec<(String, String)>,
    body_bytes: Vec<u8>,
    is_streaming: bool,
    backend: &Backend,
    config: &PipelineConfig,
    ctx: &mut PipelineContext,
) -> Result<reqwest::Response, ProxyError> {
    // Validate backend is configured
    if !backend.is_configured() {
        let err = ProxyError::BackendNotConfigured {
            backend: backend.name.clone(),
            reason: "api_key is not set".to_string(),
        };
        ctx.observability.finish_error(ctx.span.clone(), Some(err.status_code().as_u16()));
        ctx.span_finalized = true;
        return Err(err);
    }

    let path_and_query = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let upstream_uri = format!("{}{}", backend.base_url, path_and_query);

    let mut attempt = 0u32;

    let upstream_resp = loop {
        let mut builder = client.request(method.clone(), &upstream_uri);

        // Add all headers
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }

        // For streaming requests: skip reqwest timeout entirely.
        // connect_timeout is set on Client, idle_timeout on ObservedStream.
        // For non-streaming: apply request timeout to the full response.
        if !is_streaming {
            builder = builder.timeout(config.timeout_config.request);
        }

        let send_result = builder.body(body_bytes.clone()).send().await;

        match send_result {
            Ok(response) => break response,
            Err(err) => {
                crate::metrics::app_log_error(
                    "upstream",
                    &format!(
                        "Upstream request error details: backend='{}', is_connect={}, is_timeout={}, is_request={}, is_body={}",
                        backend.name,
                        err.is_connect(),
                        err.is_timeout(),
                        err.is_request(),
                        err.is_body()
                    ),
                    &format!("{:?}", err),
                );

                let should_retry = err.is_connect() || err.is_timeout();
                if should_retry && attempt < config.pool_config.max_retries {
                    let backoff = config
                        .pool_config
                        .retry_backoff_base
                        .saturating_mul(1u32 << attempt);
                    crate::metrics::app_log(
                        "upstream",
                        &format!(
                            "Upstream request failed, retrying: backend='{}', attempt={}/{}, backoff_ms={}, error={}",
                            backend.name,
                            attempt + 1,
                            config.pool_config.max_retries,
                            backoff.as_millis(),
                            err
                        ),
                    );
                    sleep(backoff).await;
                    attempt += 1;
                    continue;
                }

                if err.is_timeout() {
                    let timeout_err = ProxyError::RequestTimeout {
                        duration: config.timeout_config.request.as_secs(),
                    };
                    let mut span = ctx.span.clone();
                    span.mark_timed_out();
                    ctx.observability.finish_error(span, Some(timeout_err.status_code().as_u16()));
                    ctx.span_finalized = true;
                    return Err(timeout_err);
                }

                let conn_err = ProxyError::ConnectionError {
                    backend: backend.name.clone(),
                    source: err,
                };
                ctx.observability.finish_error(ctx.span.clone(), Some(conn_err.status_code().as_u16()));
                ctx.span_finalized = true;
                return Err(conn_err);
            }
        }
    };

    Ok(upstream_resp)
}
