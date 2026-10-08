//! The async execution engine.
//!
//! GPUI drives its own executor and has no tokio reactor, while `reqwest`
//! needs one. [`HttpEngine`] therefore owns a small dedicated tokio runtime
//! (two worker threads by default) and hands back a [`RequestHandle`] whose
//! futures are executor-agnostic: the UI simply `.await`s them from a GPUI
//! task. Nothing tokio-specific crosses the crate boundary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use davi_core::env::VarScope;
use davi_core::model::{HttpMethod, HttpRequest};
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{Client, Method, multipart};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{oneshot, watch};
use tokio::task::AbortHandle;

use crate::error::NetError;
use crate::prepare::{MultipartValue, PreparedBody, PreparedRequest, prepare};
use crate::response::{HttpResponse, Progress, ResponseCookie, ResponseSize, Timings};

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Whole-request timeout. `None` waits forever (useful for SSE/long-poll).
    pub timeout: Option<Duration>,
    pub connect_timeout: Duration,
    /// Bodies larger than this are truncated to protect memory.
    pub max_body_bytes: usize,
    pub max_redirects: usize,
    pub accept_invalid_certs: bool,
    pub user_agent: String,
    pub worker_threads: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(60)),
            connect_timeout: Duration::from_secs(10),
            max_body_bytes: 64 * 1024 * 1024,
            max_redirects: 10,
            accept_invalid_certs: false,
            user_agent: concat!("davi/", env!("CARGO_PKG_VERSION")).to_owned(),
            worker_threads: 2,
        }
    }
}

/// Cheap to clone; all clones share one connection pool and cookie jar.
#[derive(Clone)]
pub struct HttpEngine {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    config: EngineConfig,
    runtime: RuntimeSlot,
}

enum RuntimeSlot {
    Owned(Option<Runtime>),
    Borrowed(Handle),
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Never block app shutdown on in-flight requests.
        if let RuntimeSlot::Owned(rt) = &mut self.runtime
            && let Some(rt) = rt.take()
        {
            rt.shutdown_background();
        }
    }
}

impl HttpEngine {
    /// Create an engine with its own background runtime.
    pub fn new(config: EngineConfig) -> Result<Self, NetError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.worker_threads.max(1))
            .thread_name("davi-net")
            .enable_all()
            .build()
            .map_err(NetError::Runtime)?;
        let client = {
            let _guard = runtime.enter();
            build_client(&config)?
        };
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                config,
                runtime: RuntimeSlot::Owned(Some(runtime)),
            }),
        })
    }

    /// Create an engine that spawns onto an existing tokio runtime.
    pub fn with_handle(handle: Handle, config: EngineConfig) -> Result<Self, NetError> {
        let client = {
            let _guard = handle.enter();
            build_client(&config)?
        };
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                config,
                runtime: RuntimeSlot::Borrowed(handle),
            }),
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }

    fn handle(&self) -> &Handle {
        match &self.inner.runtime {
            RuntimeSlot::Owned(rt) => rt
                .as_ref()
                .expect("runtime alive while engine alive")
                .handle(),
            RuntimeSlot::Borrowed(h) => h,
        }
    }

    /// Interpolate and send `request`. Safe to call from any thread/executor.
    pub fn send(&self, request: &HttpRequest, vars: &VarScope) -> RequestHandle {
        match prepare(request, vars) {
            Ok(prepared) => self.send_prepared(prepared),
            Err(e) => RequestHandle::failed(e),
        }
    }

    pub fn send_prepared(&self, prepared: PreparedRequest) -> RequestHandle {
        let (result_tx, result_rx) = oneshot::channel();
        let (progress_tx, progress_rx) = watch::channel(Progress::Connecting);
        // Clone only what the task needs, never `Inner`: the runtime must not
        // be dropped from one of its own worker threads.
        let client = self.inner.client.clone();
        let config = self.inner.config.clone();
        let task = self.handle().spawn(async move {
            let result = execute(&client, &config, prepared, &progress_tx).await;
            let _ = progress_tx.send(Progress::Done);
            let _ = result_tx.send(result);
        });
        RequestHandle {
            result: result_rx,
            progress: progress_rx,
            abort: Some(task.abort_handle()),
        }
    }
}

/// An in-flight request. All methods are runtime-agnostic.
pub struct RequestHandle {
    result: oneshot::Receiver<Result<HttpResponse, NetError>>,
    progress: watch::Receiver<Progress>,
    abort: Option<AbortHandle>,
}

impl RequestHandle {
    fn failed(error: NetError) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(Err(error));
        Self {
            result: rx,
            progress: watch::channel(Progress::Done).1,
            abort: None,
        }
    }

    /// Abort the request; [`RequestHandle::response`] then yields `Cancelled`.
    pub fn canceller(&self) -> Canceller {
        Canceller(self.abort.clone())
    }

    /// Subscribe to streaming progress updates.
    pub fn progress(&self) -> watch::Receiver<Progress> {
        self.progress.clone()
    }

    pub async fn response(self) -> Result<HttpResponse, NetError> {
        self.result.await.unwrap_or(Err(NetError::Cancelled))
    }
}

#[derive(Clone)]
pub struct Canceller(Option<AbortHandle>);

impl Canceller {
    pub fn cancel(&self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

fn build_client(config: &EngineConfig) -> Result<Client, NetError> {
    let mut builder = Client::builder()
        .user_agent(&config.user_agent)
        .cookie_store(true)
        .connect_timeout(config.connect_timeout)
        .redirect(reqwest::redirect::Policy::limited(config.max_redirects))
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(4)
        .tcp_nodelay(true)
        .danger_accept_invalid_certs(config.accept_invalid_certs);
    if let Some(timeout) = config.timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(NetError::from)
}

fn to_method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
        HttpMethod::Head => Method::HEAD,
        HttpMethod::Options => Method::OPTIONS,
        HttpMethod::Connect => Method::CONNECT,
        HttpMethod::Trace => Method::TRACE,
    }
}

async fn execute(
    client: &Client,
    config: &EngineConfig,
    prepared: PreparedRequest,
    progress: &watch::Sender<Progress>,
) -> Result<HttpResponse, NetError> {
    let mut builder = client.request(to_method(prepared.method), prepared.url);

    for (name, value) in &prepared.headers {
        let header_name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|e| NetError::InvalidHeader {
                name: name.clone(),
                reason: e.to_string(),
            })?;
        let header_value = HeaderValue::from_str(value).map_err(|e| NetError::InvalidHeader {
            name: name.clone(),
            reason: e.to_string(),
        })?;
        builder = builder.header(header_name, header_value);
    }
    if let Some((user, pass)) = &prepared.basic_auth {
        builder = builder.basic_auth(user, Some(pass));
    }
    builder = match prepared.body {
        PreparedBody::None => builder,
        PreparedBody::Raw(body) => builder.body(body),
        PreparedBody::Form(pairs) => builder.form(&pairs),
        PreparedBody::Multipart(parts) => builder.multipart(build_multipart(parts).await?),
    };

    let started = Instant::now();
    let response = builder.send().await?;
    let ttfb = started.elapsed();

    let status = response.status();
    let _ = progress.send(Progress::Headers {
        status: status.as_u16(),
        ttfb,
    });

    let version = format!("{:?}", response.version());
    let url = response.url().to_string();
    let content_length = response.content_length();
    let mut header_bytes = 16u64; // status line
    let mut headers = Vec::with_capacity(response.headers().len());
    let mut cookies = Vec::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        header_bytes += (name.as_str().len() + value.len() + 4) as u64;
        if name == reqwest::header::SET_COOKIE {
            cookies.extend(ResponseCookie::parse(&value));
        }
        headers.push((name.as_str().to_owned(), value));
    }

    // Stream the body chunk by chunk: progress stays live for large payloads
    // and we can stop at the memory cap instead of buffering unbounded data.
    let capacity = content_length
        .unwrap_or(8 * 1024)
        .min(config.max_body_bytes as u64);
    let mut body = Vec::with_capacity(capacity as usize);
    let mut truncated = false;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let room = config.max_body_bytes - body.len();
        if chunk.len() > room {
            body.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
        let _ = progress.send(Progress::Receiving {
            received: body.len() as u64,
            total: content_length,
        });
    }
    let total = started.elapsed();

    Ok(HttpResponse {
        status: status.as_u16(),
        reason: status.canonical_reason(),
        version,
        headers,
        cookies,
        size: ResponseSize {
            headers: header_bytes,
            body: body.len() as u64,
        },
        body: body.into(),
        truncated,
        timings: Timings { ttfb, total },
        url,
    })
}

async fn build_multipart(
    parts: Vec<crate::prepare::MultipartPart>,
) -> Result<multipart::Form, NetError> {
    let mut form = multipart::Form::new();
    for part in parts {
        form = match part.value {
            MultipartValue::Text(text) => form.text(part.name, text),
            MultipartValue::File(path) => {
                let bytes = tokio::fs::read(&path)
                    .await
                    .map_err(|source| NetError::File {
                        path: path.clone(),
                        source,
                    })?;
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                form.part(
                    part.name,
                    multipart::Part::bytes(bytes).file_name(file_name),
                )
            }
        };
    }
    Ok(form)
}
