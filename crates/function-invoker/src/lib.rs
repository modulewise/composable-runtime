//! Wraps a `composable:runtime/function` for a caller that has a request type
//! with body and headers. It merges configured request headers into the body,
//! calls the function, and then extracts configured response headers from
//! result fields. The function to call is provided as a closure, which enables
//! both host-side and inter-component calls.

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

/// Headers to map into params or out of results.
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
/// and return the result after splitting out any configured response headers.
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

/// Prepare the JSON string for a function call by combining the body's fields
/// with any configured request headers. Does not fail on missing headers since
/// that should be handled by the target, which knows what fields are required.
fn merge_input(
    body: Option<&str>,
    headers: &[(String, String)],
    mapping: &HeaderMapping,
) -> Result<String, Error> {
    let mut params = params_from(body)?;

    for (param, source) in &mapping.request_headers {
        // A param mapped from a header should not also be in the body.
        if params.contains_key(param) {
            return Err(Error::InvalidRequest(format!(
                "`{param}` is supplied by the `{source}` header, so the body must not provide it"
            )));
        }
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
    let parsed = serde_json::from_str(&output)
        .map_err(|e| Error::InvalidResponse(format!("not valid JSON: {e}")))?;

    // Only an object has fields to extract and only if mappings are non-empty.
    let (Value::Object(mut fields), false) = (parsed, mapping.response_headers.is_empty()) else {
        return Ok(Response {
            body: output,
            headers: Vec::new(),
        });
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

/// The function's params as read from the request body, empty if the body is
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A mapping from `dest = source` pairs, as a config would supply them.
    fn mapping(request: &[(&str, &str)], response: &[(&str, &str)]) -> HeaderMapping {
        let pairs = |from: &[(&str, &str)]| {
            from.iter()
                .map(|(dest, source)| (dest.to_string(), source.to_string()))
                .collect()
        };
        HeaderMapping {
            request_headers: pairs(request),
            response_headers: pairs(response),
        }
    }

    fn headers(from: &[(&str, &str)]) -> Vec<(String, String)> {
        from.iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_named_header_merges_into_a_mapped_param() {
        // The param name is the destination, the header name the source.
        let merged = merge_input(
            Some(r#"{"city":"New York"}"#),
            &headers(&[("traceparent", "00-abc-def-01")]),
            &mapping(&[("trace", "traceparent")], &[]),
        )
        .expect("merge");
        assert_eq!(merged, r#"{"city":"New York","trace":"00-abc-def-01"}"#);
    }

    #[test]
    fn a_missing_header_does_not_cause_failure() {
        // The component will handle, based on whether it's required.
        let merged = merge_input(
            Some(r#"{"city":"Chicago"}"#),
            &headers(&[]),
            &mapping(&[("trace", "traceparent")], &[]),
        )
        .expect("merge");
        assert_eq!(merged, r#"{"city":"Chicago"}"#);
    }

    #[test]
    fn an_all_headers_mapping_provides_a_single_object() {
        let merged = merge_input(
            None,
            &headers(&[("x-request-id", "42"), ("traceparent", "00-abc")]),
            &mapping(&[("meta", ALL_HEADERS)], &[]),
        )
        .expect("merge");
        assert_eq!(
            merged,
            r#"{"meta":{"traceparent":"00-abc","x-request-id":"42"}}"#
        );
    }

    #[test]
    fn a_param_cannot_be_provided_by_both_body_and_headers() {
        // Params mapped from request headers should be filtered out of the
        // input schema so that a caller would not provide those same params
        // in the body, but this is a backstop.
        let error = merge_input(
            Some(r#"{"trace":"from-body"}"#),
            &headers(&[("traceparent", "from-header")]),
            &mapping(&[("trace", "traceparent")], &[]),
        )
        .expect_err("the body must not provide it");
        assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    }

    #[test]
    fn a_body_providing_a_header_mapped_param_is_rejected() {
        // The body is invalid even if the mapped header is not available.
        let error = merge_input(
            Some(r#"{"trace":"from-body"}"#),
            &headers(&[]),
            &mapping(&[("trace", "traceparent")], &[]),
        )
        .expect_err("the body must not provide it");
        assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    }

    #[test]
    fn a_body_that_is_absent_or_empty_provides_no_params() {
        // This is what a target taking no params expects.
        for body in [None, Some(""), Some("   ")] {
            let merged =
                merge_input(body, &headers(&[]), &HeaderMapping::default()).expect("merge");
            assert_eq!(merged, "{}", "body {body:?}");
        }
    }

    #[test]
    fn a_body_that_is_not_an_object_is_an_invalid_request() {
        for body in ["42", r#""text""#, "[1,2]", "{oops"] {
            let error = merge_input(Some(body), &headers(&[]), &HeaderMapping::default())
                .expect_err("not params");
            assert!(
                matches!(error, Error::InvalidRequest(_)),
                "body {body}: {error:?}"
            );
        }
    }

    #[test]
    fn a_mapped_response_field_moves_from_the_body_to_a_header() {
        let response = split_output(
            r#"{"result":"ok","request-id":"abc"}"#.to_string(),
            &mapping(&[], &[("x-request-id", "request-id")]),
        )
        .expect("split");
        // Extracted, not copied.
        assert_eq!(response.body, r#"{"result":"ok"}"#);
        assert_eq!(response.headers, headers(&[("x-request-id", "abc")]));
    }

    #[test]
    fn a_mapped_response_field_the_function_did_not_return_is_skipped() {
        let response = split_output(
            r#"{"result":"ok"}"#.to_string(),
            &mapping(&[], &[("x-duration", "elapsed-ms")]),
        )
        .expect("split");
        assert_eq!(response.body, r#"{"result":"ok"}"#);
        assert!(response.headers.is_empty(), "{:?}", response.headers);
    }

    #[test]
    fn mapped_response_fields_become_string_headers() {
        let response = split_output(
            r#"{"id":"abc","elapsed-ms":17,"ok":true}"#.to_string(),
            &mapping(
                &[],
                &[("x-id", "id"), ("x-duration", "elapsed-ms"), ("x-ok", "ok")],
            ),
        )
        .expect("split");
        assert_eq!(
            response.headers,
            headers(&[("x-duration", "17"), ("x-id", "abc"), ("x-ok", "true")])
        );
    }

    #[test]
    fn a_result_passes_through_as_is_when_no_mapping_is_configured() {
        for output in [r#"{"result":"ok"}"#, "42", r#""text""#, "null"] {
            let response =
                split_output(output.to_string(), &HeaderMapping::default()).expect("split");
            assert_eq!(response.body, output);
            assert!(response.headers.is_empty());
        }
    }

    #[test]
    fn a_result_that_is_not_an_object_passes_through() {
        let response =
            split_output("42".to_string(), &mapping(&[], &[("x-id", "id")])).expect("split");
        assert_eq!(response.body, "42");
        assert!(response.headers.is_empty());
    }

    #[test]
    fn a_result_that_is_not_json_is_invalid() {
        for mapping in [HeaderMapping::default(), mapping(&[], &[("x-id", "id")])] {
            let error = split_output("not json".to_string(), &mapping).expect_err("not JSON");
            assert!(matches!(error, Error::InvalidResponse(_)), "{error:?}");
        }
    }

    /// Run a future that never pends. The only await in `invoke` is the call
    /// closure, which these tests answer immediately, so no executor is needed.
    fn ready<F: Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            |_| RawWaker::new(std::ptr::null(), &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
        match std::pin::pin!(future).poll(&mut Context::from_waker(&waker)) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("the test's call closure must answer immediately"),
        }
    }

    #[test]
    fn invoke_handles_both_request_and_response_mappings() {
        let mapping = mapping(&[("trace", "traceparent")], &[("x-duration", "elapsed-ms")]);
        let request = Request {
            body: Some(r#"{"city":"Denver"}"#.to_string()),
            headers: headers(&[("traceparent", "00-abc")]),
        };

        let response = ready(invoke(request, &mapping, |input| {
            // The function sees the body plus the mapped header.
            assert_eq!(input, r#"{"city":"Denver","trace":"00-abc"}"#);
            async { Ok(r#"{"forecast":"snow","elapsed-ms":17}"#.to_string()) }
        }))
        .expect("invoke");

        assert_eq!(response.body, r#"{"forecast":"snow"}"#);
        assert_eq!(response.headers, headers(&[("x-duration", "17")]));
    }

    #[test]
    fn a_function_call_error_propagates_as_failed_call() {
        let error = ready(invoke(
            Request::default(),
            &HeaderMapping::default(),
            |_| async { Err("target says no".to_string()) },
        ))
        .expect_err("target says no");
        assert!(matches!(error, Error::FailedCall(_)), "{error:?}");
        // The target's error message reaches the caller.
        assert_eq!(error.to_string(), "target says no");
    }

    #[test]
    fn an_invalid_request_fails_before_the_function_is_called() {
        let mut called = false;
        let error = ready(invoke(
            Request {
                body: Some("42".to_string()),
                headers: Vec::new(),
            },
            &HeaderMapping::default(),
            |_| {
                called = true;
                async { Ok(String::new()) }
            },
        ))
        .expect_err("not params");
        assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
        assert!(
            !called,
            "the function must not be called with an invalid request"
        );
    }
}
