wit_bindgen::generate!({
    path: "../../wit",
    world: "translating-greeter",
    async: false,
    generate_all
});

use modulewise::examples::translator;

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> String {
        let message = format!("Hello, {}!", name);
        translator::translate(&message, "haw-US")
    }
}

export!(Greeter);
