use url::Url;

wit_bindgen::generate!({
    path: "../wit",
    generate_all,
});

use exports::composable::http::client::{Guest, HttpResponse, Method, RequestOptions};

use wasi::http::types::{
    ErrorCode, Fields, Method as WasiMethod, Request as WasiRequest,
    RequestOptions as WasiRequestOptions, Response as WasiResponse, Scheme,
};
use wit_bindgen::rt::async_support::{FutureReader, StreamReader};

struct HttpClient;

impl HttpClient {
    async fn request(
        method: WasiMethod,
        url: String,
        headers: Vec<(String, String)>,
        body: StreamReader<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        let request_headers = Fields::new();
        for (name, value) in &headers {
            request_headers
                .append(name, value.as_bytes())
                .map_err(|e| format!("Invalid request header {name:?}: {e:?}"))?;
        }

        let parsed = Url::parse(&url).map_err(|e| format!("Invalid URL: {e}"))?;
        let scheme = match parsed.scheme() {
            "http" => Scheme::Http,
            "https" => Scheme::Https,
            other => return Err(format!("Unsupported URL scheme: {other}")),
        };
        let host = parsed
            .host_str()
            .ok_or_else(|| "URL is missing a host".to_string())?;
        let authority = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let path_with_query = match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_string(),
        };

        let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
        trailers_tx.write(Ok(None));

        let wasi_options = options.map(wasi_request_options).transpose()?;
        let (request, send_result) =
            WasiRequest::new(request_headers, Some(body), trailers_rx, wasi_options);
        request
            .set_method(&method)
            .map_err(|()| "Failed to set request method".to_string())?;
        request
            .set_scheme(Some(&scheme))
            .map_err(|()| "Failed to set request scheme".to_string())?;
        request
            .set_authority(Some(&authority))
            .map_err(|()| "Failed to set request authority".to_string())?;
        request
            .set_path_with_query(Some(&path_with_query))
            .map_err(|()| "Failed to set request path".to_string())?;

        let response = wasi::http::client::send(request)
            .await
            .map_err(|e| format!("HTTP request failed: {e:?}"))?;

        let status = response.get_status_code();
        let headers = read_fields(&response.get_headers());

        let (body_stream, wasi_trailers) = WasiResponse::consume_body(response, send_result);

        Ok(HttpResponse {
            status,
            headers,
            body: body_stream,
            trailers: map_trailers(wasi_trailers),
        })
    }
}

fn read_fields(fields: &Fields) -> Vec<(String, String)> {
    fields
        .copy_all()
        .into_iter()
        .map(|(k, v)| (k, v.into_iter().map(|b| b as char).collect()))
        .collect()
}

fn map_trailers(
    wasi: FutureReader<Result<Option<wasi::http::types::Trailers>, ErrorCode>>,
) -> FutureReader<Result<Vec<(String, String)>, String>> {
    let (tx, rx) = wit_future::new(|| Ok(Vec::new()));
    wit_bindgen::rt::async_support::spawn_local(async move {
        let resolved = match wasi.await {
            Ok(Some(t)) => Ok(read_fields(&t)),
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("wasi:http error: {e:?}")),
        };
        tx.write(resolved);
    });
    rx
}

fn wasi_request_options(opts: RequestOptions) -> Result<WasiRequestOptions, String> {
    let r = WasiRequestOptions::new();
    if let Some(ms) = opts.connect_timeout_ms {
        r.set_connect_timeout(Some(ms_to_ns(ms)))
            .map_err(|e| format!("connect-timeout: {e:?}"))?;
    }
    if let Some(ms) = opts.first_byte_timeout_ms {
        r.set_first_byte_timeout(Some(ms_to_ns(ms)))
            .map_err(|e| format!("first-byte-timeout: {e:?}"))?;
    }
    if let Some(ms) = opts.between_bytes_timeout_ms {
        r.set_between_bytes_timeout(Some(ms_to_ns(ms)))
            .map_err(|e| format!("between-bytes-timeout: {e:?}"))?;
    }
    Ok(r)
}

fn ms_to_ns(ms: u32) -> u64 {
    u64::from(ms) * 1_000_000
}

fn to_wasi_method(method: Method) -> WasiMethod {
    match method {
        Method::Get => WasiMethod::Get,
        Method::Post => WasiMethod::Post,
        Method::Put => WasiMethod::Put,
        Method::Delete => WasiMethod::Delete,
        Method::Patch => WasiMethod::Patch,
        Method::Head => WasiMethod::Head,
        Method::Options => WasiMethod::Options,
    }
}

fn empty_body() -> StreamReader<u8> {
    let (tx, rx) = wit_stream::new::<u8>();
    drop(tx);
    rx
}

impl Guest for HttpClient {
    async fn request(
        method: Method,
        url: String,
        headers: Vec<(String, String)>,
        body: StreamReader<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(to_wasi_method(method), url, headers, body, options).await
    }

    async fn get(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Get, url, headers, empty_body(), options).await
    }

    async fn post(
        url: String,
        headers: Vec<(String, String)>,
        body: StreamReader<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Post, url, headers, body, options).await
    }

    async fn put(
        url: String,
        headers: Vec<(String, String)>,
        body: StreamReader<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Put, url, headers, body, options).await
    }

    async fn delete(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Delete, url, headers, empty_body(), options).await
    }

    async fn patch(
        url: String,
        headers: Vec<(String, String)>,
        body: StreamReader<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Patch, url, headers, body, options).await
    }

    async fn head(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Head, url, headers, empty_body(), options).await
    }

    async fn options(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(WasiMethod::Options, url, headers, empty_body(), options).await
    }
}

export!(HttpClient);
