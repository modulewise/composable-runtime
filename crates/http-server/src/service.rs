use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use composable_runtime::{ComponentHost, ConfigHandler, MessagePublisher, Service};

use crate::config::{self, HttpServerConfigHandler, ServerConfig, SharedConfig};
use crate::server::HttpServer;

/// HTTP Server support for the composable runtime.
///
/// Register with `RuntimeBuilder::with_service::<HttpService>()`.
/// Handles `[server.*]` definitions where `type = "http"`.
pub struct HttpService {
    servers: SharedConfig,
    component_host: Mutex<Option<Arc<dyn ComponentHost>>>,
    message_publisher: Mutex<Option<Arc<dyn MessagePublisher>>>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for HttpService {
    fn default() -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            servers: config::shared_config(),
            component_host: Mutex::new(None),
            message_publisher: Mutex::new(None),
            shutdown_tx,
            shutdown_rx,
            tasks: Mutex::new(Vec::new()),
        }
    }
}

impl Service for HttpService {
    fn config_handler(&self) -> Option<Box<dyn ConfigHandler>> {
        Some(Box::new(HttpServerConfigHandler::new(Arc::clone(
            &self.servers,
        ))))
    }

    fn set_component_host(&self, component_host: Arc<dyn ComponentHost>) {
        *self.component_host.lock().unwrap() = Some(component_host);
    }

    fn set_message_publisher(&self, message_publisher: Arc<dyn MessagePublisher>) {
        *self.message_publisher.lock().unwrap() = Some(message_publisher);
    }

    fn start(&self) -> Result<()> {
        let component_host = self
            .component_host
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("HttpService: component host not set"))?;

        let message_publisher = self.message_publisher.lock().unwrap().clone();

        let servers: Vec<ServerConfig> = {
            let mut lock = self.servers.lock().unwrap();
            std::mem::take(&mut *lock)
        };

        if servers.is_empty() {
            return Ok(());
        }

        // Bind every port before spawning any server, so a port that is taken
        // fails startup with nothing left running.
        let mut bound = Vec::with_capacity(servers.len());
        for config in servers {
            let name = config.name.clone();
            let port = config.port;
            let server = HttpServer::new(
                config,
                Arc::clone(&component_host),
                message_publisher.clone(),
            )?;
            let listener = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| {
                anyhow::anyhow!("HTTP server '{name}' cannot bind port {port}: {e}")
            })?;
            listener.set_nonblocking(true)?;
            bound.push((name, port, server, listener));
        }

        let mut tasks = self.tasks.lock().unwrap();
        for (name, port, server, listener) in bound {
            tracing::info!(server = %name, port, "starting HTTP server");

            let shutdown = self.shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = server.run(listener, shutdown).await {
                    tracing::error!(server = %name, "HTTP server error: {e}");
                }
            }));
        }

        Ok(())
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {
            let _ = self.shutdown_tx.send(true);
            let tasks: Vec<_> = {
                let mut lock = self.tasks.lock().unwrap();
                std::mem::take(&mut *lock)
            };
            for task in tasks {
                let _ = task.await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use composable_runtime::{Component, ComponentInstance, ComponentInvoker, Selector, Val};

    use super::*;

    // A host with no components, for servers with no routes.
    struct EmptyHost;

    impl ComponentInvoker for EmptyHost {
        fn invoke<'a>(
            &'a self,
            _component_name: &'a str,
            _function_name: &'a str,
            _args: Vec<Val>,
            _env: Option<HashMap<String, String>>,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Val>>> + Send + 'a>> {
            Box::pin(async { anyhow::bail!("no components") })
        }
    }

    impl ComponentHost for EmptyHost {
        fn get_component(&self, _name: &str) -> Option<&Component> {
            None
        }

        fn list_components(&self, _selector: Option<&Selector>) -> Vec<&Component> {
            Vec::new()
        }

        fn instantiate<'a>(
            &'a self,
            _component_name: &'a str,
            _env: Option<HashMap<String, String>>,
        ) -> Pin<Box<dyn Future<Output = Result<ComponentInstance>> + Send + 'a>> {
            Box::pin(async { anyhow::bail!("no components") })
        }
    }

    fn server(name: &str, port: u16) -> ServerConfig {
        ServerConfig {
            name: name.to_string(),
            port,
            routes: Vec::new(),
            otlp_endpoint: None,
            otlp_protocol: "grpc".to_string(),
        }
    }

    fn service(servers: Vec<ServerConfig>) -> HttpService {
        let service = HttpService::default();
        service.set_component_host(Arc::new(EmptyHost));
        service.servers.lock().unwrap().extend(servers);
        service
    }

    #[tokio::test]
    async fn a_taken_port_fails_start_and_starts_no_server() {
        // A free port, released so that both servers ask for it.
        let port = std::net::TcpListener::bind(("0.0.0.0", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let service = service(vec![server("first", port), server("second", port)]);

        let error = service
            .start()
            .expect_err("the second server's port is taken");

        assert!(
            error.to_string().contains("'second' cannot bind port"),
            "{error}"
        );
        assert!(service.tasks.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_started_server_serves_until_shutdown() {
        // Port 0 binds any free port.
        let service = service(vec![server("only", 0)]);

        service.start().expect("start");
        assert_eq!(service.tasks.lock().unwrap().len(), 1);

        service.shutdown().await;
        assert!(service.tasks.lock().unwrap().is_empty());
    }
}
