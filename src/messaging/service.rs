use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::config::types::{CategoryClaim, ConfigHandler, PropertyMap};
use crate::service::Service;
use crate::types::{ComponentInvoker, MessagePublisher};

use super::bus::{Bus, LocalBus, LocalChannelFactory, SubscriptionConfig};
use super::message::MessageBuilder;

// Claims the `subscription` property on the `component` category.
struct MessagingConfigHandler {
    subscriptions: Arc<Mutex<Vec<SubscriptionConfig>>>,
}

impl ConfigHandler for MessagingConfigHandler {
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([("component", ["subscription"].as_slice())])
    }

    fn handle_category(
        &mut self,
        category: &str,
        _name: &str,
        _properties: PropertyMap,
    ) -> Result<()> {
        anyhow::bail!("MessagingConfigHandler does not own category '{category}'")
    }

    fn handle_properties(
        &mut self,
        _category: &str,
        name: &str,
        properties: PropertyMap,
    ) -> Result<()> {
        if let Some(value) = properties.get("subscription") {
            let channel_name = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("subscription must be a string"))?
                .to_string();
            self.subscriptions.lock().unwrap().push(SubscriptionConfig {
                component_name: name.to_string(),
                channel_name,
            });
        }
        Ok(())
    }
}

pub(crate) struct MessagingService {
    subscriptions: Arc<Mutex<Vec<SubscriptionConfig>>>,
    // Currently one default bus using LocalChannel.
    // Future: HashMap<String, Arc<dyn Bus>> populated from [bus.*] config.
    bus: Arc<dyn Bus>,
}

impl MessagingService {
    pub(crate) fn new() -> Self {
        Self {
            subscriptions: Arc::new(Mutex::new(Vec::new())),
            bus: Arc::new(LocalBus::new(LocalChannelFactory)),
        }
    }

    pub(crate) fn publisher(&self) -> Arc<dyn MessagePublisher> {
        Arc::new(BusPublisher {
            bus: Arc::clone(&self.bus),
        })
    }
}

impl Service for MessagingService {
    fn config_handler(&self) -> Option<Box<dyn ConfigHandler>> {
        Some(Box::new(MessagingConfigHandler {
            subscriptions: Arc::clone(&self.subscriptions),
        }))
    }

    fn set_invoker(&self, invoker: Arc<dyn ComponentInvoker>) {
        self.bus.set_invoker(invoker);
    }

    fn start(&self) -> Result<()> {
        let subscriptions: Vec<_> = self.subscriptions.lock().unwrap().drain(..).collect();
        for sub in subscriptions {
            self.bus.add_subscription(sub);
        }
        self.bus.start()
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.bus.shutdown()
    }
}

// Implements MessagePublisher by delegating to the bus.
struct BusPublisher {
    bus: Arc<dyn Bus>,
}

impl MessagePublisher for BusPublisher {
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        body: Vec<u8>,
        headers: HashMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut builder = MessageBuilder::new(body);
            for (key, value) in headers {
                builder = builder.header(key, value);
            }
            let msg = builder.build();
            self.bus.publish(channel, msg).await
        })
    }
}
