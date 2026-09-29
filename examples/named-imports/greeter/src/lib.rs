wit_bindgen::generate!({
    path: "../wit",
    world: "greeter",
});

struct Greeter;

impl Guest for Greeter {
    fn greet(name: String) -> Vec<String> {
        let greeting = "hello";
        vec![
            format!("{greeting} {name}!"),
            format!(
                "{} {}!",
                spanish::translate(greeting),
                spanish::translate(&name)
            ),
            format!(
                "{} {}!",
                french::translate(greeting),
                french::translate(&name)
            ),
        ]
    }
}

export!(Greeter);
