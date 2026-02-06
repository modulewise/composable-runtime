wit_bindgen::generate!({
    path: "../wit",
    world: "greeter",
    generate_all
});

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        let greeting = wasi::config::store::get("greeting")
            .ok()
            .flatten()
            .unwrap_or_else(|| "Hello".to_string());
        format!("{}, {}!", greeting, name)
    }
}

export!(Greeter);
