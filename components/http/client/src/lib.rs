use url::Url;
use wasi::http::types::{
    Headers, IncomingBody, Method as WasiMethod, OutgoingBody, OutgoingRequest,
    RequestOptions as WasiRequestOptions, Scheme,
};
use wasi::io::streams::StreamError;

wit_bindgen::generate!({
    path: "../wit",
    generate_all
});

use exports::composable::http::client::{Guest, HttpResponse, Method, RequestOptions};

struct HttpClient;

impl HttpClient {
    fn request(
        method: &WasiMethod,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        let request_headers = Headers::new();
        for (name, value) in headers {
            request_headers
                .append(&name, value.as_bytes())
                .map_err(|e| format!("Failed to set header {name}: {e:?}"))?;
        }

        let parsed = Url::parse(url).map_err(|e| format!("Invalid URL: {e}"))?;
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

        let request = OutgoingRequest::new(request_headers);
        request
            .set_method(method)
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

        if let Some(body_data) = body {
            let outgoing_body = request
                .body()
                .map_err(|()| "Failed to get request body".to_string())?;
            let output_stream = outgoing_body
                .write()
                .map_err(|()| "Failed to get output stream".to_string())?;
            output_stream
                .blocking_write_and_flush(&body_data)
                .map_err(|e| format!("Failed to write request body: {e:?}"))?;
            drop(output_stream);
            OutgoingBody::finish(outgoing_body, None)
                .map_err(|e| format!("wasi:http error: {e:?}"))?;
        }

        let max_response_body_bytes = options.as_ref().and_then(|o| o.max_response_body_bytes);
        let wasi_options = options.map(wasi_request_options).transpose()?;
        let response = wasi::http::outgoing_handler::handle(request, wasi_options)
            .map_err(|_| "Failed to send HTTP request".to_string())?;
        response.subscribe().block();
        let response = response
            .get()
            .ok_or("Failed to get response".to_string())?
            .map_err(|()| "Request failed".to_string())?
            .map_err(|e| format!("wasi:http error: {e:?}"))?;

        let status = response.status();

        let headers: Vec<(String, String)> = response
            .headers()
            .entries()
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|b| b as char).collect()))
            .collect();

        if let Some(max) = max_response_body_bytes
            && let Some(content_length) = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse::<u64>().ok())
            && content_length > max
        {
            return Err(format!(
                "Response Content-Length {content_length} exceeds max size of {max} bytes"
            ));
        }

        let body = response
            .consume()
            .map_err(|()| "Failed to consume response body".to_string())?;
        let stream = body
            .stream()
            .map_err(|()| "Failed to get response stream".to_string())?;
        let mut body_bytes = Vec::new();
        loop {
            match stream.blocking_read(8192) {
                Ok(chunk) if chunk.is_empty() => break,
                Ok(chunk) => {
                    body_bytes.extend_from_slice(&chunk);
                    if let Some(max) = max_response_body_bytes
                        && body_bytes.len() as u64 > max
                    {
                        return Err(format!("Response body exceeded max size of {max} bytes"));
                    }
                }
                Err(StreamError::Closed) => break,
                Err(StreamError::LastOperationFailed(io_error)) => {
                    let detail = wasi::http::types::http_error_code(&io_error)
                        .map(|code| format!("wasi:http error: {code:?}"))
                        .unwrap_or_else(|| format!("stream error: {io_error:?}"));
                    return Err(format!("Failed to read response body: {detail}"));
                }
            }
        }
        drop(stream);

        let trailers = IncomingBody::finish(body);
        trailers.subscribe().block();
        let trailers: Vec<(String, String)> = trailers
            .get()
            .ok_or("Failed to get trailers".to_string())?
            .map_err(|()| "Trailers already consumed".to_string())?
            .map_err(|e| format!("wasi:http error: {e:?}"))?
            .map(|t| {
                t.entries()
                    .into_iter()
                    .map(|(k, v)| (k, v.into_iter().map(|b| b as char).collect()))
                    .collect()
            })
            .unwrap_or_default();

        Ok(HttpResponse {
            status,
            headers,
            body: body_bytes,
            trailers,
        })
    }
}

fn wasi_request_options(opts: RequestOptions) -> Result<WasiRequestOptions, String> {
    let r = WasiRequestOptions::new();
    if let Some(ms) = opts.connect_timeout_ms {
        r.set_connect_timeout(Some(ms_to_ns(ms)))
            .map_err(|()| "connect-timeout not supported by host".to_string())?;
    }
    if let Some(ms) = opts.first_byte_timeout_ms {
        r.set_first_byte_timeout(Some(ms_to_ns(ms)))
            .map_err(|()| "first-byte-timeout not supported by host".to_string())?;
    }
    if let Some(ms) = opts.between_bytes_timeout_ms {
        r.set_between_bytes_timeout(Some(ms_to_ns(ms)))
            .map_err(|()| "between-bytes-timeout not supported by host".to_string())?;
    }
    Ok(r)
}

fn ms_to_ns(ms: u32) -> u64 {
    u64::from(ms) * 1_000_000
}

impl Guest for HttpClient {
    fn request(
        method: Method,
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        let wasi_method = match method {
            Method::Get => WasiMethod::Get,
            Method::Post => WasiMethod::Post,
            Method::Put => WasiMethod::Put,
            Method::Delete => WasiMethod::Delete,
            Method::Patch => WasiMethod::Patch,
            Method::Head => WasiMethod::Head,
            Method::Options => WasiMethod::Options,
        };
        let body = if body.is_empty() { None } else { Some(body) };
        Self::request(&wasi_method, &url, headers, body, options)
    }

    fn get(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Get, &url, headers, None, options)
    }

    fn post(
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Post, &url, headers, Some(body), options)
    }

    fn put(
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Put, &url, headers, Some(body), options)
    }

    fn delete(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Delete, &url, headers, None, options)
    }

    fn patch(
        url: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Patch, &url, headers, Some(body), options)
    }

    fn head(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Head, &url, headers, None, options)
    }

    fn options(
        url: String,
        headers: Vec<(String, String)>,
        options: Option<RequestOptions>,
    ) -> Result<HttpResponse, String> {
        Self::request(&WasiMethod::Options, &url, headers, None, options)
    }
}

export!(HttpClient);
