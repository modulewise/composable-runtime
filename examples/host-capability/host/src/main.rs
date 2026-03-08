use anyhow::Result;
use composable_runtime::{ComponentState, HostCapability, Runtime};
use serde::Deserialize;
use wasmtime::component::{HasSelf, Linker};

// Generate host-side bindings for the greeting interface.
wasmtime::component::bindgen!({
    path: "../wit/host-greeting.wit",
    world: "greeter",
});

// Implement the host greeting trait on ComponentState.
impl crate::example::greeting::host_greeting::Host for ComponentState {
    fn get_greeting(&mut self) -> String {
        "Hello".to_string()
    }
}

// The host side greeting capability.
#[derive(Deserialize, Default)]
struct GreetingCapability;

impl HostCapability for GreetingCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["example:greeting/host-greeting".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> Result<()> {
        crate::example::greeting::host_greeting::add_to_linker::<_, HasSelf<_>>(linker, |state| {
            state
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = Runtime::builder()
        .from_path(std::path::PathBuf::from("config.toml"))
        .with_capability::<GreetingCapability>("greeting")
        .build()
        .await?;

    let result = runtime
        .invoke("greeter", "greet", vec![serde_json::json!("World")])
        .await?;

    println!("Result: {}", result);

    Ok(())
}
