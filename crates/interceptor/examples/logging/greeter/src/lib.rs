wit_bindgen::generate!({
    path: "../../wit",
    world: "greeter-world",
    generate_all
});

struct Greeter;

impl exports::modulewise::examples::greeter::Guest for Greeter {
    fn say_hello(name: String) -> String {
        format!("Hello {name}!")
    }

    fn say_goodbye(name: String) -> String {
        format!("Goodbye {name}!")
    }
}

export!(Greeter);
