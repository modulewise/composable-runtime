wit_bindgen::generate!({
    path: "../../wit",
    world: "simple-greeter",
    generate_all
});

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        format!("Hello, {}!", name)
    }
}

export!(Greeter);
