wit_bindgen::generate!({
    path: "../wit",
    world: "dictionary",
    generate_all
});

use exports::example::named_imports::translator::Guest;

struct Dictionary;

impl Guest for Dictionary {
    fn translate(word: String) -> String {
        wasi::config::store::get(&word.to_lowercase())
            .ok()
            .flatten()
            .unwrap_or(word)
    }
}

export!(Dictionary);
