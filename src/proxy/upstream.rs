use hyper::{Request, Response};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HOST, CONTENT_TYPE};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use anyhow::Result;

pub struct UpstreamClient {
    client: Client<HttpConnector, Full<Bytes>>,
    upstream_url: String,
}

impl UpstreamClient {
    pub fn new() -> Self {
        let connector = HttpConnector::new();
        let client = Client::builder(TokioExecutor::new()).build(connector);

        Self {
            client,
            upstream_url: "https://api.anthropic.com".to_string(),
        }
    }

    pub async fn forward(&self, req: Request<Incoming>) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let method = req.method().clone();
        let uri = req.uri();
        let path_and_query = uri.path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let upstream_uri = format!("{}{}", self.upstream_url, path_and_query);

        let mut builder = Request::builder()
            .method(method)
            .uri(upstream_uri);

        for (name, value) in req.headers() {
            if name != HOST {
                builder = builder.header(name, value);
            }
        }

        let body_bytes = req.into_body().collect().await?.to_bytes();

        let upstream_req = builder.body(Full::new(body_bytes))?;
        let upstream_resp = self.client.request(upstream_req).await?;

        let content_type = upstream_resp.headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());

        let is_streaming = content_type.map_or(false, |ct| ct.contains("text/event-stream"));

        let status = upstream_resp.status();
        let mut builder = hyper::Response::builder().status(status);

        for (name, value) in upstream_resp.headers() {
            builder = builder.header(name, value);
        }

        if is_streaming {
            Ok(builder.body(upstream_resp.into_body().boxed())?)
        } else {
            let body_bytes = upstream_resp.into_body().collect().await?.to_bytes();
            Ok(builder.body(Full::new(body_bytes).map_err(|e| match e {}).boxed())?)
        }
    }
}

impl Default for UpstreamClient {
    fn default() -> Self {
        Self::new()
    }
}
