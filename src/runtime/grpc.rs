//! gRPC transport support for wasi:http.
//!
//! When a component sends an outgoing HTTP request with
//! `content-type:application/grpc`, the `h2c-for-grpc` hook enables HTTP/2
//! instead of HTTP/1.1. Only plaintext HTTP/2 (h2c / prior knowledge) is
//! supported; gRPC over TLS requires a future update.

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;

// === p2 ======================================================================

use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as ErrorCodeP2;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{
    HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig,
};

pub(crate) fn send_grpc_request_p2(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HostFutureIncomingResponse {
    let handle = wasmtime_wasi::runtime::spawn(async move {
        Ok(send_grpc_request_p2_handler(request, config).await)
    });
    HostFutureIncomingResponse::pending(handle)
}

async fn send_grpc_request_p2_handler(
    mut request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> Result<IncomingResponse, ErrorCodeP2> {
    use http_body_util::BodyExt;
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::io::TokioIo;

    let authority = grpc_authority(request.uri()).ok_or(ErrorCodeP2::HttpRequestUriInvalid)?;

    let start = tokio::time::Instant::now();

    let tcp_stream = timeout(config.connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCodeP2::ConnectionTimeout)?
        .map_err(|_| ErrorCodeP2::ConnectionRefused)?;

    let remaining = config.connect_timeout.saturating_sub(start.elapsed());
    let tcp_stream = TokioIo::new(tcp_stream);

    let (mut sender, conn) = timeout(
        remaining,
        hyper::client::conn::http2::handshake(TokioExec, tcp_stream),
    )
    .await
    .map_err(|_| ErrorCodeP2::ConnectionTimeout)?
    .map_err(wasmtime_wasi_http::p2::hyper_request_error)?;

    let worker = wasmtime_wasi::runtime::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!("h2 connection error: {e}");
        }
    });

    strip_authority(&mut request).map_err(|_| ErrorCodeP2::HttpRequestUriInvalid)?;

    let resp = timeout(config.first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCodeP2::ConnectionReadTimeout)?
        .map_err(wasmtime_wasi_http::p2::hyper_request_error)?
        .map(|body| {
            body.map_err(wasmtime_wasi_http::p2::hyper_request_error)
                .boxed_unsync()
        });

    Ok(IncomingResponse {
        resp,
        worker: Some(worker),
        between_bytes_timeout: config.between_bytes_timeout,
    })
}

// === p3 ======================================================================

use std::future::Future;
use std::time::Duration;
use wasmtime_wasi::TrappableError;
use wasmtime_wasi_http::p3::RequestOptions;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

type P3Body = UnsyncBoxBody<Bytes, ErrorCode>;
type P3IoFuture = Box<dyn Future<Output = Result<(), ErrorCode>> + Send>;
type P3SendOutput = Result<(http::Response<P3Body>, P3IoFuture), TrappableError<ErrorCode>>;

// Defaults when `request-options` omits a timeout.
// Mirror the fallbacks used by `p3::default_send_request`.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_BETWEEN_BYTES_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) fn send_grpc_request_p3(
    request: http::Request<P3Body>,
    options: Option<RequestOptions>,
) -> Box<dyn Future<Output = P3SendOutput> + Send> {
    Box::new(send_grpc_request_p3_handler(request, options))
}

async fn send_grpc_request_p3_handler(
    mut request: http::Request<P3Body>,
    options: Option<RequestOptions>,
) -> P3SendOutput {
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::io::TokioIo;

    let connect_timeout = options
        .as_ref()
        .and_then(|o| o.connect_timeout)
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT);
    let first_byte_timeout = options
        .as_ref()
        .and_then(|o| o.first_byte_timeout)
        .unwrap_or(DEFAULT_FIRST_BYTE_TIMEOUT);
    let between_bytes_timeout = options
        .as_ref()
        .and_then(|o| o.between_bytes_timeout)
        .unwrap_or(DEFAULT_BETWEEN_BYTES_TIMEOUT);

    let authority = grpc_authority(request.uri()).ok_or(ErrorCode::HttpRequestUriInvalid)?;

    let start = tokio::time::Instant::now();

    let tcp_stream = timeout(connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|_| ErrorCode::ConnectionRefused)?;

    let remaining = connect_timeout.saturating_sub(start.elapsed());
    let tcp_stream = TokioIo::new(tcp_stream);

    let (mut sender, conn) = timeout(
        remaining,
        hyper::client::conn::http2::handshake::<_, _, P3Body>(TokioExec, tcp_stream),
    )
    .await
    .map_err(|_| ErrorCode::ConnectionTimeout)?
    .map_err(ErrorCode::from_hyper_request_error)?;

    strip_authority(&mut request).map_err(|_| ErrorCode::HttpRequestUriInvalid)?;

    use http_body_util::BodyExt;

    // Map errors to `ErrorCode` here so both poll_fn arms share one type. The
    // response body is wrapped in `TimeoutBody` to enforce between-bytes timeout.
    let send = async move {
        let res = timeout(first_byte_timeout, sender.send_request(request))
            .await
            .map_err(|_| ErrorCode::ConnectionReadTimeout)?
            .map_err(ErrorCode::from_hyper_request_error)?;
        let mut interval = tokio::time::interval(between_bytes_timeout);
        interval.reset();
        Ok(res.map(|incoming| TimeoutBody { incoming, interval }.boxed_unsync()))
    };

    // The hyper connection must be polled to drive HTTP/2 I/O. Poll the send
    // future and, while it is pending, drive `conn` so the exchange can make
    // progress (mirrors `p3::default_send_request`). `conn` is then returned as
    // the io future to drive the response body.
    let mut send = std::pin::pin!(send);
    let mut conn = Some(conn);
    let resp = std::future::poll_fn(|cx| match send.as_mut().poll(cx) {
        std::task::Poll::Ready(res) => std::task::Poll::Ready(res),
        std::task::Poll::Pending => {
            let Some(fut) = conn.as_mut() else {
                return std::task::Poll::Pending;
            };
            let res = std::task::ready!(std::pin::Pin::new(fut).poll(cx));
            conn = None;
            match res {
                Ok(()) => send.as_mut().poll(cx),
                Err(e) => std::task::Poll::Ready(Err(ErrorCode::from_hyper_request_error(e))),
            }
        }
    })
    .await?;

    let conn_fut: P3IoFuture = Box::new(async move {
        if let Some(conn) = conn {
            conn.await.map_err(hyper_response_error)?;
        }
        Ok(())
    });

    Ok((resp, conn_fut))
}

// Wraps the response body to enforce the between-bytes timeout. The interval
// resets on each received frame. If no frame arrives before it ticks, the body
// fails with `ConnectionReadTimeout`. Mirrors `default_send_request`.
struct TimeoutBody {
    incoming: hyper::body::Incoming,
    interval: tokio::time::Interval,
}

impl http_body::Body for TimeoutBody {
    type Data = <hyper::body::Incoming as http_body::Body>::Data;
    type Error = ErrorCode;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use std::task::Poll;
        match std::pin::Pin::new(&mut self.as_mut().incoming).poll_frame(cx) {
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(hyper_response_error(e)))),
            Poll::Ready(Some(Ok(frame))) => {
                self.interval.reset();
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => {
                std::task::ready!(self.interval.poll_tick(cx));
                Poll::Ready(Some(Err(ErrorCode::ConnectionReadTimeout)))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.incoming.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.incoming.size_hint()
    }
}

// Map a hyper error from the response phase to a wasi-http `ErrorCode`.
// Mirrors wasmtime's `from_hyper_response_error`, which is not public.
fn hyper_response_error(err: hyper::Error) -> ErrorCode {
    use std::error::Error as _;
    if err.is_timeout() {
        return ErrorCode::HttpResponseTimeout;
    }
    if let Some(cause) = err.source()
        && let Some(code) = cause.downcast_ref::<ErrorCode>()
    {
        return code.clone();
    }
    tracing::warn!("hyper response error: {err:?}");
    ErrorCode::HttpProtocolError
}

// === shared ==================================================================

// Build a connect authority (host:port), defaulting to port 80 for h2c.
fn grpc_authority(uri: &http::Uri) -> Option<String> {
    let authority = uri.authority()?;
    Some(if authority.port().is_some() {
        authority.to_string()
    } else {
        format!("{}:80", authority)
    })
}

// HTTP/2 carries the target in pseudo-headers, so strip scheme and authority
// from the request URI, leaving only path-and-query.
fn strip_authority<B>(request: &mut http::Request<B>) -> Result<(), http::Error> {
    *request.uri_mut() = http::Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/"),
        )
        .build()?;
    Ok(())
}

#[derive(Clone)]
struct TokioExec;

impl<F> hyper::rt::Executor<F> for TokioExec
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        tokio::spawn(fut);
    }
}
