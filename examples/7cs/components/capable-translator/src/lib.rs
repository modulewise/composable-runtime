wit_bindgen::generate!({
    path: "../../wit",
    world: "capable-translator",
    generate_all
});

use wasi::http::outgoing_handler;
use wasi::http::types::{Headers, Method, OutgoingBody, OutgoingRequest, Scheme};

struct Translator;

impl exports::modulewise::examples::translator::Guest for Translator {
    fn translate(text: String, locale: String) -> String {
        match call_translate_api(&text, &locale) {
            Ok(translated) => translated,
            Err(e) => {
                eprintln!("translate-api error: {e}");
                text
            }
        }
    }
}

fn call_translate_api(text: &str, locale: &str) -> Result<String, String> {
    let body_json = format!(r#"{{"text":"{text}","locale":"{locale}"}}"#);

    let headers = Headers::new();
    headers
        .append("content-type", b"application/json")
        .map_err(|e| format!("header error: {e:?}"))?;

    let request = OutgoingRequest::new(headers);
    request
        .set_method(&Method::Post)
        .map_err(|()| "set_method failed")?;
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|()| "set_scheme failed")?;
    request
        .set_authority(Some("localhost:8090"))
        .map_err(|()| "set_authority failed")?;
    request
        .set_path_with_query(Some("/translate"))
        .map_err(|()| "set_path failed")?;

    let outgoing_body = request.body().map_err(|e| format!("body error: {e:?}"))?;
    let output_stream = outgoing_body.write().map_err(|()| "write error")?;
    output_stream
        .blocking_write_and_flush(body_json.as_bytes())
        .map_err(|e| format!("write error: {e:?}"))?;
    drop(output_stream);
    OutgoingBody::finish(outgoing_body, None).map_err(|e| format!("finish error: {e:?}"))?;

    let future_response =
        outgoing_handler::handle(request, None).map_err(|e| format!("send error: {e:?}"))?;
    future_response.subscribe().block();

    let response = future_response
        .get()
        .ok_or("response not ready")?
        .map_err(|()| "request failed")?
        .map_err(|e| format!("HTTP error: {e:?}"))?;

    let status = response.status();
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }

    let incoming_body = response.consume().map_err(|()| "consume error")?;
    let input_stream = incoming_body.stream().map_err(|()| "stream error")?;
    let mut bytes = Vec::new();
    while let Ok(chunk) = input_stream.blocking_read(4096) {
        bytes.extend_from_slice(&chunk);
    }
    drop(input_stream);

    let resp_text = String::from_utf8(bytes).map_err(|e| format!("UTF-8 error: {e}"))?;
    // Response is {"translated": "..."}
    let parsed: serde_json::Value =
        serde_json::from_str(&resp_text).map_err(|e| format!("JSON parse error: {e}"))?;
    parsed["translated"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("unexpected response: {resp_text}"))
}

export!(Translator);
