//! Request-Reply support for messaging.
//!
//! Creates an ephemeral reply channel, provides its address for the
//! `reply-to` header, and offers a handle to receive the reply.

use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use uuid::Uuid;

use crate::message::Message;

use super::channel::{Channel, ChannelRegistry, LocalChannel, Overflow};

// Manages ephemeral reply channels for request-reply messaging. Each channel
// is cleaned up after the reply is received or when the handle is dropped.
#[derive(Clone)]
pub(crate) struct ReplyHandler {
    registry: Arc<ChannelRegistry<LocalChannel>>,
}

// Consumer group name used for reply channels.
const REPLY_GROUP: &str = "reply";

// Default timeout for waiting on a reply (30 seconds).
const DEFAULT_REPLY_TIMEOUT_MS: i64 = 30_000;

impl ReplyHandler {
    // Create a new `ReplyHandler` with its own ephemeral channel registry.
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(ChannelRegistry::new()),
        }
    }

    pub(crate) fn registry(&self) -> Arc<ChannelRegistry<LocalChannel>> {
        Arc::clone(&self.registry)
    }

    // Set up an ephemeral reply channel and return its address and a
    // handle for receiving the reply. The address should be placed in
    // the `reply-to` header of the outgoing request message.
    pub(crate) fn return_address(&self) -> Box<dyn crate::message::ReturnAddress> {
        let name = format!("reply-{}", Uuid::new_v4());
        let channel = Arc::new(LocalChannel::new(
            1,
            Overflow::Block,
            DEFAULT_REPLY_TIMEOUT_MS,
            DEFAULT_REPLY_TIMEOUT_MS,
        ));
        channel.init_group(REPLY_GROUP);
        self.registry.register(&name, Arc::clone(&channel));

        Box::new(ChannelReturnAddress {
            channel,
            name,
            registry: Arc::clone(&self.registry),
        })
    }
}

// A return address for receiving a reply. Use `channel()` to get the name for
// the `reply-to` header. Consuming the reply via `take` automatically removes
// the ephemeral channel. Otherwise, removal happens in the `Drop` impl.
pub(crate) struct ChannelReturnAddress {
    channel: Arc<LocalChannel>,
    name: String,
    registry: Arc<ChannelRegistry<LocalChannel>>,
}

impl crate::message::ReturnAddress for ChannelReturnAddress {
    fn channel(&self) -> &str {
        &self.name
    }

    fn take(
        self: Box<Self>,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<Message, anyhow::Error>> + Send>> {
        Box::pin(async move {
            let result = self.channel.consume(REPLY_GROUP).await;
            self.registry.remove(&self.name);
            let (msg, _receipt) =
                result.map_err(|e| anyhow::anyhow!("reply channel error: {e}"))?;
            Ok(msg)
        })
    }
}

impl Drop for ChannelReturnAddress {
    fn drop(&mut self) {
        self.registry.remove(&self.name);
    }
}
