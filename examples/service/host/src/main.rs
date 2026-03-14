use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use composable_runtime::{
    CategoryClaim, ComponentState, ConfigHandler, HostCapability, HostCapabilityFactory,
    PropertyMap, Runtime, Service, create_capability, create_state,
};
use wasmtime::component::{HasSelf, Linker};

// Generate host-side bindings for the greeting interface.
wasmtime::component::bindgen!({
    path: "../wit/greeter.wit",
    world: "greeter",
});

// Implement the host greeting trait on ComponentState.
impl crate::example::greeter::host_greeting::Host for ComponentState {
    fn get_greeting(&mut self) -> String {
        self.get_extension::<GreetingState>()
            .map(|s| s.message.clone())
            .unwrap_or_else(|| "Hello".to_string())
    }
}

// Per-instance state storing the final greeting message.
struct GreetingState {
    message: String,
}

// The host-side greeting capability.
// message comes from [greeting] config (via service's Arc<Mutex>).
// uppercase comes from [capability.greeting] config.* sub-table.
struct GreetingCapability {
    message: String,
    uppercase: bool,
}

impl HostCapability for GreetingCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["example:greeter/host-greeting".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        crate::example::greeter::host_greeting::add_to_linker::<_, HasSelf<_>>(linker, |state| {
            state
        })
    }

    create_state!(this, GreetingState, {
        let message = if this.uppercase {
            this.message.to_uppercase()
        } else {
            this.message.clone()
        };
        GreetingState { message }
    });
}

// Config parsed from [greeting] category, shared between handler and service.
struct GreetingConfig {
    message: String,
}

// ConfigHandler that parses the [greeting] category.
struct GreetingConfigHandler {
    config: Arc<Mutex<Option<GreetingConfig>>>,
}

impl ConfigHandler for GreetingConfigHandler {
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![CategoryClaim::all("greeting")]
    }

    fn handle_category(
        &mut self,
        category: &str,
        _name: &str,
        properties: PropertyMap,
    ) -> Result<()> {
        assert_eq!(category, "greeting");
        let message = properties
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Hello")
            .to_string();
        *self.config.lock().unwrap() = Some(GreetingConfig { message });
        Ok(())
    }
}

// Service: owns a [greeting] config category, provides a greeting capability.
struct GreetingService {
    config: Arc<Mutex<Option<GreetingConfig>>>,
}

impl Default for GreetingService {
    fn default() -> Self {
        Self {
            config: Arc::new(Mutex::new(None)),
        }
    }
}

impl Service for GreetingService {
    fn config_handler(&self) -> Option<Box<dyn ConfigHandler>> {
        Some(Box::new(GreetingConfigHandler {
            config: Arc::clone(&self.config),
        }))
    }

    fn capabilities(&self) -> Vec<(&'static str, HostCapabilityFactory)> {
        // Take config out of the mutex — no contention after this point.
        let config = self.config.lock().unwrap().take();
        let message = config.map(|c| c.message).unwrap_or_else(|| "Hello".into());
        let message = Arc::new(message);

        vec![create_capability!("greeting", |config| {
            let uppercase = config
                .get("uppercase")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            GreetingCapability {
                message: (*message).clone(),
                uppercase,
            }
        })]
    }

    fn start(&self) -> Result<()> {
        println!("[GreetingService] started");
        Ok(())
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        println!("[GreetingService] shutdown");
        Box::pin(async {})
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let runtime = Runtime::builder()
        .from_path(std::path::PathBuf::from("config.toml"))
        .with_service::<GreetingService>()
        .build()
        .await?;

    runtime.start()?;

    let result = runtime
        .invoke("greeter", "greet", vec![serde_json::json!("World")])
        .await?;

    println!("Result: {result}");

    runtime.shutdown().await;

    Ok(())
}
