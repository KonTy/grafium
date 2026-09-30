use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use reqwest::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use url::Url;

use crate::error::{Result, RuntimeError};

pub(crate) type Cancel = Option<Arc<AtomicBool>>;

/// Authorization hook checked before every request, including health checks.
/// This validates URLs, not resolved addresses. See the module's network limits.
pub trait NetworkPolicy: Send + Sync {
    fn authorize(&self, url: &Url) -> Result<()>;
}

impl<F: Fn(&Url) -> Result<()> + Send + Sync> NetworkPolicy for F {
    fn authorize(&self, url: &Url) -> Result<()> {
        self(url)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ResponseLimits {
    /// Total bytes received per JSON response or stream.
    pub body_bytes: usize,
    /// Maximum SSE event / NDJSON line, including protocol metadata.
    pub frame_bytes: usize,
    /// Maximum accumulated completion text.
    pub output_bytes: usize,
}

impl Default for ResponseLimits {
    fn default() -> Self {
        Self {
            body_bytes: 16 * 1024 * 1024,
            frame_bytes: 1024 * 1024,
            output_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Explicit host-owned HTTP configuration. The default authorizes configured
/// HTTP(S) URLs, without assuming that a provider named "Ollama" is private.
#[derive(Clone)]
pub struct NetworkConfig {
    client: Client,
    policy: Arc<dyn NetworkPolicy>,
    limits: ResponseLimits,
}

impl NetworkConfig {
    pub fn new(timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(http_error)?;
        Ok(Self {
            client,
            policy: Arc::new(|_: &Url| Ok(())),
            limits: ResponseLimits::default(),
        })
    }

    /// The host is responsible for this client's redirect, DNS, retry and proxy
    /// settings. Use `ClientBuilder::no_proxy()` when proxy inheritance is unsafe.
    pub fn from_client(client: Client, policy: Arc<dyn NetworkPolicy>) -> Self {
        Self {
            client,
            policy,
            limits: ResponseLimits::default(),
        }
    }

    pub fn with_policy(mut self, policy: Arc<dyn NetworkPolicy>) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_limits(mut self, limits: ResponseLimits) -> Result<Self> {
        if limits.body_bytes == 0 || limits.frame_bytes == 0 || limits.output_bytes == 0 {
            return Err(RuntimeError::Other(
                "HTTP response limits must be positive".into(),
            ));
        }
        self.limits = limits;
        Ok(self)
    }
}

pub(crate) struct Endpoint {
    base: Url,
    pub config: NetworkConfig,
}

impl Endpoint {
    pub fn new(base: &str, config: NetworkConfig) -> Result<Self> {
        let mut base = Url::parse(base)
            .map_err(|_| RuntimeError::Other("Invalid provider base URL".into()))?;
        validate_url(&base)?;
        if base.query().is_some() || base.fragment().is_some() {
            return Err(RuntimeError::Other(
                "Provider base URL must not contain a query or fragment".into(),
            ));
        }
        let path = format!("{}/", base.path().trim_end_matches('/'));
        base.set_path(&path);
        config.policy.authorize(&base)?;
        Ok(Self { base, config })
    }

    pub fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        // All paths are static suffixes supplied by the transport, not by input.
        self.config
            .client
            .request(method, format!("{}{path}", self.base))
    }

    pub fn is_github_models(&self) -> bool {
        self.base.host_str() == Some("models.github.ai") && self.base.path() == "/inference/"
    }

    pub fn limits(&self) -> ResponseLimits {
        self.config.limits
    }

    pub async fn send(&self, request: RequestBuilder, cancel: &Cancel) -> Result<Response> {
        check_cancel(cancel)?;
        let request = request.build().map_err(http_error)?;
        validate_url(request.url())?;
        self.config.policy.authorize(request.url())?;
        cancellable(self.config.client.execute(request), cancel).await
    }
}

fn validate_url(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RuntimeError::Other(
            "Provider URL requires HTTP(S), a host and no embedded credentials".into(),
        ));
    }
    Ok(())
}

fn http_error(error: reqwest::Error) -> RuntimeError {
    RuntimeError::Other(format!(
        "Model HTTP transport failed: {}",
        error.without_url()
    ))
}

pub(crate) fn check_cancel(cancel: &Cancel) -> Result<()> {
    if cancel
        .as_ref()
        .is_some_and(|flag| flag.load(Ordering::Acquire))
    {
        Err(RuntimeError::Cancelled)
    } else {
        Ok(())
    }
}

async fn cancellable<T>(
    future: impl Future<Output = reqwest::Result<T>>,
    cancel: &Cancel,
) -> Result<T> {
    check_cancel(cancel)?;
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(20)), if cancel.is_some() => check_cancel(cancel)?,
            result = &mut future => {
                check_cancel(cancel)?;
                return result.map_err(http_error);
            }
        }
    }
}

pub(crate) fn success(response: Response) -> Result<Response> {
    if !response.status().is_success() {
        // Do not echo arbitrary server bodies (possibly containing prompt data).
        return Err(RuntimeError::Other(format!(
            "Model endpoint returned HTTP {}",
            response.status()
        )));
    }
    Ok(response)
}

pub(crate) async fn json<T: DeserializeOwned>(
    mut response: Response,
    limits: ResponseLimits,
    cancel: &Cancel,
) -> Result<T> {
    if response
        .content_length()
        .is_some_and(|size| size > limits.body_bytes as u64)
    {
        return Err(RuntimeError::Other(
            "Model response exceeds body limit".into(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = cancellable(response.chunk(), cancel).await? {
        if chunk.len() > limits.body_bytes.saturating_sub(body.len()) {
            return Err(RuntimeError::Other(
                "Model response exceeds body limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    check_cancel(cancel)?;
    let parsed = serde_json::from_slice(&body)?;
    check_cancel(cancel)?;
    Ok(parsed)
}

pub(crate) fn is_json(response: &Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| {
            h.split(';')
                .next()
                .is_some_and(|v| v.trim() == "application/json")
        })
}

pub(crate) fn append(
    text: &mut String,
    delta: &str,
    limits: ResponseLimits,
    cancel: &Cancel,
    on_token: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    check_cancel(cancel)?;
    if delta.len() > limits.output_bytes.saturating_sub(text.len()) {
        return Err(RuntimeError::Other(
            "Model output exceeds text limit".into(),
        ));
    }
    text.push_str(delta);
    if !delta.is_empty() {
        on_token(delta);
    }
    check_cancel(cancel)
}

pub(crate) fn completion(text: String, limits: ResponseLimits) -> Result<String> {
    if text.is_empty() {
        return Err(RuntimeError::Other(
            "Provider returned empty completion".into(),
        ));
    }
    if text.len() > limits.output_bytes {
        return Err(RuntimeError::Other(
            "Model output exceeds text limit".into(),
        ));
    }
    Ok(text)
}

/// Incremental framing with bounded unfinished lines, events and total bytes.
/// EOF is not a successful terminal event; each provider must require its own.
pub(crate) async fn frames(
    mut response: Response,
    sse: bool,
    limits: ResponseLimits,
    cancel: &Cancel,
    mut on_frame: impl FnMut(&str) -> Result<bool>,
) -> Result<()> {
    let mut line = Vec::new();
    let mut event = String::new();
    let mut total = 0usize;
    let mut event_bytes = 0usize;
    while let Some(chunk) = cancellable(response.chunk(), cancel).await? {
        if chunk.len() > limits.body_bytes.saturating_sub(total) {
            return Err(RuntimeError::Other(
                "Model stream exceeds body limit".into(),
            ));
        }
        total += chunk.len();
        for byte in chunk {
            if byte != b'\n' {
                if line.len() >= limits.frame_bytes {
                    return Err(RuntimeError::Other(
                        "Model stream exceeds frame limit".into(),
                    ));
                }
                line.push(byte);
                continue;
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let decoded = std::str::from_utf8(&line)
                .map_err(|_| RuntimeError::Other("Model stream is not UTF-8".into()))?;
            check_cancel(cancel)?;
            if sse {
                if decoded.is_empty() {
                    if !event.is_empty() && on_frame(event.trim_end_matches('\n'))? {
                        return check_cancel(cancel);
                    }
                    event.clear();
                    event_bytes = 0;
                } else {
                    event_bytes = event_bytes.saturating_add(line.len() + 1);
                    if event_bytes > limits.frame_bytes {
                        return Err(RuntimeError::Other(
                            "Model stream exceeds event limit".into(),
                        ));
                    }
                    if let Some(data) = decoded.strip_prefix("data:") {
                        event.push_str(data.strip_prefix(' ').unwrap_or(data));
                        event.push('\n');
                    }
                }
            } else if !decoded.trim().is_empty() && on_frame(decoded)? {
                return check_cancel(cancel);
            }
            line.clear();
        }
    }
    // NDJSON permits a final record without a newline. SSE requires a blank
    // line to dispatch an event; an incomplete SSE event is a truncated stream.
    if !sse && !line.is_empty() {
        let decoded = std::str::from_utf8(&line)
            .map_err(|_| RuntimeError::Other("Model stream is not UTF-8".into()))?;
        if on_frame(decoded)? {
            return check_cancel(cancel);
        }
    }
    Err(RuntimeError::Other(
        "Model stream ended before its terminal event".into(),
    ))
}
