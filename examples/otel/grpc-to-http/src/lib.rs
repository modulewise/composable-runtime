#![no_main]

wit_bindgen::generate!({
    path: "../wit",
    world: "grpc-to-http",
    generate_all,
});

use std::collections::HashMap;
use std::sync::OnceLock;

use wasi::config::store;
use wasi::http::outgoing_handler;
use wasi::http::types::{Headers, Method, OutgoingBody, OutgoingRequest, Scheme};

struct GrpcToHttp;

struct Config {
    url: String,
    paths: HashMap<String, String>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

impl Config {
    fn from_store() -> Result<Self, String> {
        let all = store::get_all().map_err(|e| format!("Failed to get config: {e:?}"))?;

        let mut url = None;
        let mut paths = HashMap::new();

        for (key, value) in all {
            if key == "url" {
                url = Some(value);
            } else if let Some(path_key) = key.strip_prefix("paths.") {
                paths.insert(path_key.to_string(), value);
            }
        }

        let url = url.ok_or_else(|| "Missing 'url' in config".to_string())?;

        Ok(Self { url, paths })
    }
}

fn config() -> &'static Config {
    CONFIG.get_or_init(|| Config::from_store().expect("Config should be available"))
}

impl exports::modulewise::grpc::endpoint::Guest for GrpcToHttp {
    fn send(path: String, data: Vec<u8>) -> Result<(), String> {
        let config = config();
        let path = config
            .paths
            .get(&path)
            .ok_or_else(|| format!("Unknown path: {}", path))?;

        send_grpc_over_http(&config.url, path, &data)
    }
}

fn send_grpc_over_http(url: &str, path: &str, data: &[u8]) -> Result<(), String> {
    // Build gRPC-framed body (0 = no compression + length as 4 big endian bytes)
    let mut body_bytes = Vec::with_capacity(5 + data.len());
    body_bytes.push(0u8);
    body_bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
    body_bytes.extend_from_slice(data);

    let (scheme, authority) = url
        .split_once("://")
        .ok_or_else(|| format!("Invalid endpoint URL: {url}"))?;
    let scheme = match scheme {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        _ => return Err(format!("Unsupported scheme: {scheme}")),
    };

    let headers = Headers::new();
    headers
        .append("content-type", b"application/grpc")
        .map_err(|_| "Failed to set content-type header".to_string())?;
    headers
        .append("te", b"trailers")
        .map_err(|_| "Failed to set te header".to_string())?;

    let request = OutgoingRequest::new(headers);
    request
        .set_method(&Method::Post)
        .map_err(|()| "Failed to set method".to_string())?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "Failed to set scheme".to_string())?;
    request
        .set_authority(Some(authority))
        .map_err(|()| "Failed to set authority".to_string())?;
    request
        .set_path_with_query(Some(path))
        .map_err(|()| "Failed to set path".to_string())?;

    let outgoing_body = request
        .body()
        .map_err(|()| "Failed to get request body".to_string())?;
    let output_stream = outgoing_body
        .write()
        .map_err(|()| "Failed to get output stream".to_string())?;
    output_stream
        .blocking_write_and_flush(&body_bytes)
        .map_err(|e| format!("Failed to write body: {e:?}"))?;
    drop(output_stream);
    OutgoingBody::finish(outgoing_body, None).map_err(|_| "Failed to finish body".to_string())?;

    // Send and wait for response
    let future_response = outgoing_handler::handle(request, None)
        .map_err(|e| format!("Failed to send request: {e:?}"))?;
    future_response.subscribe().block();

    let response = future_response
        .get()
        .ok_or("Response not ready".to_string())?
        .map_err(|()| "Request failed".to_string())?
        .map_err(|e| format!("HTTP error: {e:?}"))?;

    let status = response.status();
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }

    Ok(())
}

export!(GrpcToHttp);
