wit_bindgen::generate!({
    path: "../../wit",
    world: "capable-translator",
    generate_all
});

use wasi::http::client::send;
use wasi::http::types::{Fields, Method, Request, Response, Scheme};
use wit_bindgen::rt::async_support::spawn_local;

struct Translator;

impl exports::modulewise::examples::translator::Guest for Translator {
    async fn translate(text: String, locale: String) -> String {
        match call_translate_api(&text, &locale).await {
            Ok(translated) => translated,
            Err(e) => {
                eprintln!("translate-api error: {e}");
                text
            }
        }
    }
}

async fn call_translate_api(text: &str, locale: &str) -> Result<String, String> {
    let body_json = format!(r#"{{"text":"{text}","locale":"{locale}"}}"#);

    let headers = Fields::new();
    headers
        .append("content-type", b"application/json")
        .map_err(|e| format!("header error: {e:?}"))?;

    // Spawn the body write so wasi:http's host-side send drains concurrently.
    let (mut body_tx, body_rx) = wit_stream::new::<u8>();
    let body_bytes = body_json.into_bytes();
    spawn_local(async move {
        let _ = body_tx.write_all(body_bytes).await;
    });

    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    trailers_tx.write(Ok(None));

    let (request, send_result) = Request::new(headers, Some(body_rx), trailers_rx, None);
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

    let response = send(request)
        .await
        .map_err(|e| format!("HTTP request failed: {e:?}"))?;

    let status = response.get_status_code();
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }

    let (body_stream, _trailers) = Response::consume_body(response, send_result);
    let bytes = body_stream.collect().await;

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
