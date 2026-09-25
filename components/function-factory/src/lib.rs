wit_bindgen::generate!({
    path: "wit",
    world: "function-factory",
    generate_all,
});

struct Factory;

impl exports::composable::factory::factory::Guest for Factory {
    async fn build() -> Result<Vec<u8>, String> {
        let wit = config("wit")?.ok_or_else(|| "no `config.wit` set".to_string())?;
        let world = config("world")?;
        let function = config("function")?;
        let description = config("description")?;

        function_adapter::build(wit, world, function, description).map_err(|e| format!("{e:#}"))
    }
}

fn config(key: &str) -> Result<Option<String>, String> {
    wasi::config::store::get(key).map_err(|e| format!("reading config '{key}': {e:?}"))
}

export!(Factory);
