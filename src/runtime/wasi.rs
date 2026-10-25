//! WASI and WASI HTTP views over [`ComponentState`], and the HTTP hooks that
//! propagate context onto outgoing requests and route gRPC over h2c.

use std::collections::HashMap;
use wasmtime_wasi::{ResourceTable, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    Error as HttpError, RequestOptions, WasiBody, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
};
use wasmtime_wasi_io::IoView;

use crate::context::PROPAGATION_CONTEXT;
use crate::types::{ComponentState, HttpHooks, PROPAGATED_HEADERS};

impl IoView for ComponentState {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.resource_table
    }
}

impl WasiView for ComponentState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi_ctx,
            table: &mut self.resource_table,
        }
    }
}

impl HttpHooks {
    // Inject propagation-context headers (e.g. tracecontext) onto an outgoing
    // request. Shared by the p2 and p3 hooks.
    fn propagate_headers<B>(request: &mut http::Request<B>) {
        let ctx_entries: Option<HashMap<String, String>> = PROPAGATION_CONTEXT
            .try_with(|ctx| ctx.as_ref().map(|c| c.entries.clone()))
            .ok()
            .flatten();
        if let Some(entries) = ctx_entries {
            for key in PROPAGATED_HEADERS {
                if let Some(val) = entries.get(*key)
                    && let Ok(hv) = val.parse()
                {
                    request.headers_mut().insert(*key, hv);
                }
            }
        }
    }

    // Whether a request is gRPC (content-type application/grpc).
    fn is_grpc<B>(request: &http::Request<B>) -> bool {
        request
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/grpc"))
    }
}

type WasiIoFuture = Box<dyn std::future::Future<Output = Result<(), HttpError>> + Send>;

impl WasiHttpView for ComponentState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            hooks: &mut self.http_hooks,
            table: &mut self.resource_table,
            ctx: self.wasi_http_ctx.as_mut().expect(
                "Component requires 'http' capability, so HTTP context should be available",
            ),
        }
    }
}

impl WasiHttpHooks for HttpHooks {
    fn send_request(
        &mut self,
        mut request: http::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: WasiIoFuture,
    ) -> Box<
        dyn std::future::Future<
                Output = Result<(http::Response<WasiBody>, WasiIoFuture), HttpError>,
            > + Send,
    > {
        Self::propagate_headers(&mut request);

        if self.h2c_for_grpc && Self::is_grpc(&request) {
            if request.uri().scheme_str() == Some("https") {
                tracing::error!("h2c-for-grpc does not support TLS (https)");
                return Box::new(async move { Err(HttpError::HttpProtocolError) });
            }
            super::grpc::send_grpc_request(request, options)
        } else {
            // `fut` is the guest-side request-error channel,
            // unused by `default_send_request`.
            let _ = fut;
            Box::new(async move {
                use http_body_util::BodyExt;
                let (res, io) = wasmtime_wasi_http::default_send_request(request, options).await?;
                Ok((res.map(BodyExt::boxed_unsync), Box::new(io) as WasiIoFuture))
            })
        }
    }
}
