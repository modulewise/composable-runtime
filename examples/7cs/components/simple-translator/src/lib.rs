wit_bindgen::generate!({
    path: "../../wit",
    world: "simple-translator",
    generate_all
});

struct Translator;

impl exports::modulewise::examples::translator::Guest for Translator {
    fn translate(text: String, locale: String) -> String {
        let lang = locale.split(['_', '-']).next().unwrap_or("");
        match lang {
            "de" => text.replace("Hello", "Hallo"),
            "es" => text.replace("Hello", "Hola"),
            "fr" => text.replace("Hello", "Bonjour"),
            "haw" => text.replace("Hello", "Aloha"),
            "nl" => text.replace("Hello", "Hallo"),
            _ => text,
        }
    }
}

export!(Translator);
