//! gRPC transport support for wasi:http.
//!
//! When a component sends an outgoing HTTP request with `content-type: application/grpc`,
//! the runtime uses HTTP/2 instead of HTTP/1.1. Currently only plaintext HTTP/2
//! (h2c / prior knowledge) is supported; gRPC over TLS requires a future update.

use wasmtime_wasi_http::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig,
};

pub(crate) fn send_grpc_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HostFutureIncomingResponse {
    let handle = wasmtime_wasi::runtime::spawn(async move {
        Ok(send_grpc_request_handler(request, config).await)
    });
    HostFutureIncomingResponse::pending(handle)
}

async fn send_grpc_request_handler(
    mut request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> Result<IncomingResponse, ErrorCode> {
    use http_body_util::BodyExt;
    use tokio::net::TcpStream;
    use tokio::time::timeout;
    use wasmtime_wasi_http::io::TokioIo;

    let authority = request
        .uri()
        .authority()
        .ok_or(ErrorCode::HttpRequestUriInvalid)?;
    let authority = if authority.port().is_some() {
        authority.to_string()
    } else {
        format!("{}:80", authority)
    };

    let start = tokio::time::Instant::now();

    let tcp_stream = timeout(config.connect_timeout, TcpStream::connect(&authority))
        .await
        .map_err(|_| ErrorCode::ConnectionTimeout)?
        .map_err(|_| ErrorCode::ConnectionRefused)?;

    let remaining = config.connect_timeout.saturating_sub(start.elapsed());
    let tcp_stream = TokioIo::new(tcp_stream);

    let (mut sender, conn) = timeout(
        remaining,
        hyper::client::conn::http2::handshake(TokioExec, tcp_stream),
    )
    .await
    .map_err(|_| ErrorCode::ConnectionTimeout)?
    .map_err(wasmtime_wasi_http::hyper_request_error)?;

    let worker = wasmtime_wasi::runtime::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!("h2 connection error: {e}");
        }
    });

    // Strip scheme and authority - HTTP/2 uses pseudo-headers
    *request.uri_mut() = hyper::Uri::builder()
        .path_and_query(
            request
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/"),
        )
        .build()
        .map_err(|_| ErrorCode::HttpRequestUriInvalid)?;

    let resp = timeout(config.first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| ErrorCode::ConnectionReadTimeout)?
        .map_err(wasmtime_wasi_http::hyper_request_error)?
        .map(|body| {
            body.map_err(wasmtime_wasi_http::hyper_request_error)
                .boxed_unsync()
        });

    Ok(IncomingResponse {
        resp,
        worker: Some(worker),
        between_bytes_timeout: config.between_bytes_timeout,
    })
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
