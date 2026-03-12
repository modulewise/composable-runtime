wit_bindgen::generate!({
    path: "../../wit",
    world: "configured-greeter",
    generate_all
});

use modulewise::examples::translator;

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        let locale = wasi::config::store::get("locale")
            .ok()
            .flatten()
            .unwrap_or_else(|| "en-US".to_string());
        let message = format!("Hello, {}!", name);
        translator::translate(&message, &locale)
    }
}

export!(Greeter);
