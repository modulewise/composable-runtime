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

// Default when `request-options` does not specify a connect timeout.
// Mirrors the fallback used by `p3::default_send_request`.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(600);
const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(600);

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

    let conn_fut: P3IoFuture = Box::new(async move {
        if let Err(e) = conn.await {
            tracing::warn!("h2 connection error: {e}");
        }
        Ok(())
    });

    strip_authority(&mut request).map_err(|_| ErrorCode::HttpRequestUriInvalid)?;

    let resp = timeout(first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCode::ConnectionReadTimeout)?
        .map_err(ErrorCode::from_hyper_request_error)?;

    use http_body_util::BodyExt;
    let resp = resp.map(|body| {
        body.map_err(ErrorCode::from_hyper_request_error)
            .boxed_unsync()
    });

    Ok((resp, conn_fut))
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
