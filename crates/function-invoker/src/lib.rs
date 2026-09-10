//! Wraps a `composable:runtime/function` for a caller that has a request type
//! with body and headers. It merges configured request headers into the body,
//! calls the function, and then extracts configured result fields into the
//! configured response headers. The function to call is provided as a closure,
//! for consistent support across host-side or inter-component calls.

use std::collections::BTreeMap;
use std::future::Future;

use serde_json::{Map, Value};

/// A request as body and headers.
#[derive(Debug, Default, Clone)]
pub struct Request {
    /// The target's params as JSON, or `None` if the target has no params.
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// A response as body and headers.
#[derive(Debug, Default, Clone)]
pub struct Response {
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Headers to map into params or extract out of results.
#[derive(Debug, Default, Clone)]
pub struct HeaderMapping {
    /// `param = "header"`, or `param = *` for all headers as a map.
    pub request_headers: BTreeMap<String, String>,
    /// `header = "field"`.
    pub response_headers: BTreeMap<String, String>,
}

pub const ALL_HEADERS: &str = "*";

#[derive(Debug)]
pub enum Error {
    /// The request could not be read as function params.
    InvalidRequest(String),
    /// The function call failed.
    FailedCall(String),
    /// The function response was not valid JSON.
    InvalidResponse(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidRequest(message) => write!(f, "request body: {message}"),
            Error::FailedCall(message) => write!(f, "{message}"),
            Error::InvalidResponse(message) => write!(f, "function result: {message}"),
        }
    }
}

impl std::error::Error for Error {}

/// Call a function after merging the body with any configured request headers,
/// and return the result after extracting any configured response headers.
pub async fn invoke<F, Fut>(
    request: Request,
    mapping: &HeaderMapping,
    call: F,
) -> Result<Response, Error>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    let input = merge_input(request.body.as_deref(), &request.headers, mapping)?;
    let output = call(input).await.map_err(Error::FailedCall)?;
    split_output(output, mapping)
}

/// Prepare the JSON string for a function by combining the body's fields with
/// any configured request headers. Does not fail on missing headers since that
/// should be handled by the target, whether optional or required.
fn merge_input(
    body: Option<&str>,
    headers: &[(String, String)],
    mapping: &HeaderMapping,
) -> Result<String, Error> {
    let mut params = params_from(body)?;

    for (param, source) in &mapping.request_headers {
        // When all headers are expected on a param, they can be read as a
        // `map<string, string>` or as a record whose fields are header names.
        if source == ALL_HEADERS {
            let all: Map<String, Value> = headers
                .iter()
                .map(|(name, value)| (name.clone(), Value::String(value.clone())))
                .collect();
            params.insert(param.clone(), Value::Object(all));
            continue;
        }
        if let Some((_, value)) = headers.iter().find(|(name, _)| name == source) {
            params.insert(param.clone(), Value::String(value.clone()));
        }
    }

    Ok(Value::Object(params).to_string())
}

/// Prepare the invoker's response by extracting any configured response
/// headers from the JSON string returned by the function call.
fn split_output(output: String, mapping: &HeaderMapping) -> Result<Response, Error> {
    if mapping.response_headers.is_empty() {
        return Ok(Response {
            body: output,
            headers: Vec::new(),
        });
    }

    let mut fields = match serde_json::from_str(&output) {
        Ok(Value::Object(object)) => object,
        // Only an object has fields to extract. Anything else returns as is.
        Ok(_) => {
            return Ok(Response {
                body: output,
                headers: Vec::new(),
            });
        }
        Err(e) => return Err(Error::InvalidResponse(format!("not valid JSON: {e}"))),
    };

    let mut headers = Vec::new();
    for (header, field) in &mapping.response_headers {
        let Some(value) = fields.remove(field) else {
            continue;
        };
        // A header value is always represented as a string.
        headers.push(match value {
            Value::String(text) => (header.clone(), text),
            other => (header.clone(), other.to_string()),
        });
    }

    Ok(Response {
        body: Value::Object(fields).to_string(),
        headers,
    })
}

/// The function's params read from the request body, empty if the body is
/// itself empty or `None`.
fn params_from(body: Option<&str>) -> Result<Map<String, Value>, Error> {
    match body {
        None => Ok(Map::new()),
        Some(text) if text.trim().is_empty() => Ok(Map::new()),
        Some(text) => match serde_json::from_str(text) {
            Ok(Value::Object(object)) => Ok(object),
            Ok(other) => Err(Error::InvalidRequest(format!(
                "expected an object of params, got {other}"
            ))),
            Err(e) => Err(Error::InvalidRequest(format!("not valid JSON: {e}"))),
        },
    }
}
