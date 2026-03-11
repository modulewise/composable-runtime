use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use composable_runtime::{ComponentInvoker, ConfigHandler, MessagePublisher, Service};

use crate::config::{self, GatewayConfig, HttpGatewayConfigHandler, SharedConfig};

/// HTTP Gateway service for the composable runtime.
///
/// Register with `RuntimeBuilder::with_service::<HttpGatewayService>()`.
/// Handles `[gateway.*]` definitions where `type = "http"`.
pub struct HttpGatewayService {
    gateways: SharedConfig,
    invoker: Mutex<Option<Arc<dyn ComponentInvoker>>>,
    publisher: Mutex<Option<Arc<dyn MessagePublisher>>>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for HttpGatewayService {
    fn default() -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            gateways: config::shared_config(),
            invoker: Mutex::new(None),
            publisher: Mutex::new(None),
            shutdown_tx,
            shutdown_rx,
            tasks: Mutex::new(Vec::new()),
        }
    }
}

impl Service for HttpGatewayService {
    fn config_handler(&self) -> Option<Box<dyn ConfigHandler>> {
        Some(Box::new(HttpGatewayConfigHandler::new(Arc::clone(
            &self.gateways,
        ))))
    }

    fn set_invoker(&self, invoker: Arc<dyn ComponentInvoker>) {
        *self.invoker.lock().unwrap() = Some(invoker);
    }

    fn set_publisher(&self, publisher: Arc<dyn MessagePublisher>) {
        *self.publisher.lock().unwrap() = Some(publisher);
    }

    fn start(&self) -> Result<()> {
        let invoker = self
            .invoker
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("HttpGatewayService: invoker not set"))?;

        let publisher = self.publisher.lock().unwrap().clone();

        let gateways: Vec<GatewayConfig> = {
            let mut lock = self.gateways.lock().unwrap();
            std::mem::take(&mut *lock)
        };

        if gateways.is_empty() {
            return Ok(());
        }

        let mut tasks = self.tasks.lock().unwrap();
        for gateway in gateways {
            let invoker = Arc::clone(&invoker);
            let publisher = publisher.clone();
            let shutdown = self.shutdown_rx.clone();
            let name = gateway.name.clone();
            let port = gateway.port;

            tracing::info!(gateway = %name, port, routes = gateway.routes.len(), "starting HTTP gateway");

            tasks.push(tokio::spawn(async move {
                if let Err(e) =
                    crate::server::run(port, gateway.routes, invoker, publisher, shutdown).await
                {
                    tracing::error!(gateway = %name, "HTTP gateway error: {e}");
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
