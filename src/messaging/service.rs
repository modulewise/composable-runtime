use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::config::types::{CategoryClaim, ConfigHandler, PropertyMap};
use crate::message::{Message, MessageBuilder, MessageHeaders, MessagePublisher};
use crate::service::Service;
use crate::types::ComponentInvoker;

use super::bus::{Bus, LocalBus, LocalChannelFactory, SubscriptionConfig};
use super::reply::ReplyHandler;

// Claims the `[subscription.*]` category. Each entry connects a component to
// a channel. The entry's `channel` field defaults to the subscription name.
// An optional `mapping` declares how the message body maps to the target
// function's WIT args.
struct MessagingConfigHandler {
    subscriptions: Arc<Mutex<Vec<SubscriptionConfig>>>,
}

impl ConfigHandler for MessagingConfigHandler {
    fn claimed_categories(&self) -> Vec<CategoryClaim> {
        vec![CategoryClaim::all("subscription")]
    }

    fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
        HashMap::from([(
            "subscription",
            [
                "channel",
                "component",
                "function",
                "param-mapping",
                "param-encoding",
                "result-decoding",
                "result-mapping",
            ]
            .as_slice(),
        )])
    }

    fn handle_category(
        &mut self,
        category: &str,
        name: &str,
        mut properties: PropertyMap,
    ) -> Result<()> {
        if category != "subscription" {
            anyhow::bail!("MessagingConfigHandler does not own category '{category}'");
        }

        let component_name = match properties.remove("component") {
            Some(serde_json::Value::String(s)) => s,
            Some(other) => {
                anyhow::bail!("Subscription '{name}': 'component' must be a string, got {other}");
            }
            None => {
                anyhow::bail!("Subscription '{name}' is missing required 'component'");
            }
        };

        let channel_name = match properties.remove("channel") {
            Some(serde_json::Value::String(s)) => s,
            Some(other) => {
                anyhow::bail!("Subscription '{name}': 'channel' must be a string, got {other}");
            }
            None => name.to_string(),
        };

        let function_key = match properties.remove("function") {
            Some(serde_json::Value::String(s)) => Some(s),
            Some(other) => {
                anyhow::bail!("Subscription '{name}': 'function' must be a string, got {other}");
            }
            None => None,
        };

        let param_mapping = match properties.remove("param-mapping") {
            Some(serde_json::Value::Object(map)) => Some(map.into_iter().collect()),
            Some(other) => {
                anyhow::bail!(
                    "Subscription '{name}': 'param-mapping' must be an object, got {other}"
                );
            }
            None => None,
        };

        let param_encoding = match properties.remove("param-encoding") {
            Some(serde_json::Value::Object(map)) => Some(
                crate::mapping::ParamEncoding::parse(&map)
                    .map_err(|e| anyhow::anyhow!("Subscription '{name}': 'param-encoding': {e}"))?,
            ),
            Some(other) => {
                anyhow::bail!(
                    "Subscription '{name}': 'param-encoding' must be an object, got {other}"
                );
            }
            None => None,
        };

        // The `result-mapping` is a single template Value (not a
        // name => template map like `param-mapping`), so any JSON shape is
        // accepted: object/array templates with `{path}` placeholders, a
        // path-only template string, or a literal scalar. The `map_result`
        // function validates substitution at runtime.
        let result_mapping = properties.remove("result-mapping");

        let result_decoding = match properties.remove("result-decoding") {
            Some(serde_json::Value::Object(map)) => {
                Some(crate::mapping::ResultDecoding::parse(&map).map_err(|e| {
                    anyhow::anyhow!("Subscription '{name}': 'result-decoding': {e}")
                })?)
            }
            Some(other) => {
                anyhow::bail!(
                    "Subscription '{name}': 'result-decoding' must be an object, got {other}"
                );
            }
            None => None,
        };

        self.subscriptions.lock().unwrap().push(SubscriptionConfig {
            channel_name,
            component_name,
            function_key,
            mapping: crate::mapping::MappingConfig {
                param_mapping,
                param_encoding,
                result_decoding,
                result_mapping,
            },
        });
        Ok(())
    }
}

pub(crate) struct MessagingService {
    subscriptions: Arc<Mutex<Vec<SubscriptionConfig>>>,
    // Currently one default bus using LocalChannel.
    // Future: HashMap<String, Arc<dyn Bus>> populated from [bus.*] config.
    bus: Arc<dyn Bus>,
    reply_handler: ReplyHandler,
}

impl MessagingService {
    pub(crate) fn new() -> Self {
        Self {
            subscriptions: Arc::new(Mutex::new(Vec::new())),
            bus: Arc::new(LocalBus::new(LocalChannelFactory)),
            reply_handler: ReplyHandler::new(),
        }
    }

    pub(crate) fn publisher(&self) -> Arc<dyn MessagePublisher> {
        Arc::new(BusPublisher {
            bus: Arc::clone(&self.bus),
            reply_handler: self.reply_handler.clone(),
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
        self.bus.set_reply_publisher(self.reply_handler.registry());
        self.bus.start()
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.bus.shutdown()
    }
}

// Implements MessagePublisher by delegating to the bus.
struct BusPublisher {
    bus: Arc<dyn Bus>,
    reply_handler: ReplyHandler,
}

impl MessagePublisher for BusPublisher {
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { self.bus.publish(channel, message).await })
    }

    fn publish_request<'a>(
        &'a self,
        channel: &'a str,
        message: Message,
    ) -> crate::message::ReplyFuture<'a> {
        Box::pin(async move {
            let return_address = self.reply_handler.return_address();
            let msg = MessageBuilder::from_message(message)
                .header(MessageHeaders::REPLY_TO, return_address.channel())
                .build();
            self.bus.publish(channel, msg).await?;
            Ok(return_address)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler() -> (MessagingConfigHandler, Arc<Mutex<Vec<SubscriptionConfig>>>) {
        let subs = Arc::new(Mutex::new(Vec::new()));
        (
            MessagingConfigHandler {
                subscriptions: Arc::clone(&subs),
            },
            subs,
        )
    }

    #[test]
    fn subscription_parses_result_decoding() {
        let (mut handler, subs) = make_handler();
        let mut props = PropertyMap::new();
        props.insert(
            "component".to_string(),
            serde_json::json!("transport-client"),
        );
        props.insert(
            "result-decoding".to_string(),
            serde_json::json!({ "body": "{headers.content-type}" }),
        );
        handler
            .handle_category("subscription", "events", props)
            .unwrap();
        let subs = subs.lock().unwrap();
        assert_eq!(subs.len(), 1);
        assert!(subs[0].mapping.result_decoding.is_some());
    }

    #[test]
    fn subscription_result_decoding_non_object_is_error() {
        let (mut handler, _subs) = make_handler();
        let mut props = PropertyMap::new();
        props.insert("component".to_string(), serde_json::json!("c"));
        props.insert("result-decoding".to_string(), serde_json::json!("bad"));
        let err = handler
            .handle_category("subscription", "x", props)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be an object"), "unexpected error: {err}");
    }

    #[test]
    fn subscription_parses_param_encoding() {
        let (mut handler, subs) = make_handler();
        let mut props = PropertyMap::new();
        props.insert(
            "component".to_string(),
            serde_json::json!("transport-client"),
        );
        props.insert(
            "param-encoding".to_string(),
            serde_json::json!({ "body": "{headers.content-type}" }),
        );
        handler
            .handle_category("subscription", "events", props)
            .unwrap();
        let subs = subs.lock().unwrap();
        assert_eq!(subs.len(), 1);
        assert!(subs[0].mapping.param_encoding.is_some());
    }

    #[test]
    fn subscription_param_encoding_non_object_is_error() {
        let (mut handler, _subs) = make_handler();
        let mut props = PropertyMap::new();
        props.insert("component".to_string(), serde_json::json!("c"));
        props.insert("param-encoding".to_string(), serde_json::json!("bad"));
        let err = handler
            .handle_category("subscription", "x", props)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be an object"), "unexpected error: {err}");
    }
}
