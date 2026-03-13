wit_bindgen::generate!({
    path: "../wit/host-greeting.wit",
    world: "greeter",
    generate_all
});

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        let greeting = example::greeting::host_greeting::get_greeting();
        format!("{}, {}!", greeting, name)
    }
}

export!(Greeter);
