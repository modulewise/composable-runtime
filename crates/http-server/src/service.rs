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

        let mut tasks = self.tasks.lock().unwrap();
        for config in servers {
            let name = config.name.clone();
            let port = config.port;
            let server = HttpServer::new(
                config,
                Arc::clone(&component_host),
                message_publisher.clone(),
            )?;

            tracing::info!(server = %name, port, "starting HTTP server");

            let shutdown = self.shutdown_rx.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(e) = server.run(shutdown).await {
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
