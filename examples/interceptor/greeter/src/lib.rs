wit_bindgen::generate!({
    path: "../wit",
    world: "greeter-world",
    generate_all
});

struct Greeter;

impl exports::example::interceptor::greeter::Guest for Greeter {
    fn greet(name: String) -> String {
        format!("Hello, {name}!")
    }
}

export!(Greeter);
