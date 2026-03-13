wit_bindgen::generate!({
    path: "../../wit",
    world: "calculator-world",
    generate_all
});

struct Calculator;

impl Guest for Calculator {
    fn add(a: i32, b: i32) -> i32 {
        a + b
    }

    fn subtract(a: i32, b: i32) -> i32 {
        a - b
    }
}

export!(Calculator);
