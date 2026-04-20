use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::types::ComponentInvoker;

use super::activator::Activator;
use super::channel::{Channel, ChannelRegistry, ReplyPublisher};
use super::dispatcher::Dispatcher;
use crate::message::Message;

// Type-erasure boundary so MessagingService can hold heterogeneous buses.
pub(crate) trait Bus: Send + Sync {
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        msg: Message,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    fn add_subscription(&self, config: SubscriptionConfig);

    fn set_invoker(&self, invoker: Arc<dyn ComponentInvoker>);

    // Set the shared reply publisher that uses ephemeral reply channels.
    // The bus composes this with its own registry to build the
    // `ReplyPublisher` it gives to activators.
    fn set_reply_publisher(&self, publisher: Arc<dyn ReplyPublisher>);

    fn start(&self) -> Result<()>;

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub(crate) struct SubscriptionConfig {
    pub component_name: String,
    pub channel_name: String,
}

// Creates channel instances. The swappable part of a bus.
pub(crate) trait ChannelFactory<C: Channel>: Send + Sync {
    fn create(&self, name: &str) -> Arc<C>;

    // Called after channel creation to perform any setup that must
    // complete before the channel can accept publishes (e.g. registering
    // consumer groups for in-memory channels). Default is a no-op.
    fn init(&self, _channel: &C, _group: &str) {}

    // Called during shutdown to signal that no more messages should be
    // accepted. Default is a no-op.
    fn close(&self, _channel: &C) {}
}

// Generic bus parameterized over channel type and factory.
// All control plane logic lives here: channel creation,
// subscription resolution, and dispatcher lifecycle.
// May be type-aliased per backend: `LocalBus`, `KafkaBus`, etc.
pub(crate) struct GenericBus<C: Channel, F: ChannelFactory<C>> {
    factory: F,
    registry: Arc<ChannelRegistry<C>>,
    subscriptions: Mutex<Vec<SubscriptionConfig>>,
    invoker: Mutex<Option<Arc<dyn ComponentInvoker>>>,
    shared_reply_publisher: Mutex<Option<Arc<dyn ReplyPublisher>>>,
    cancel: CancellationToken,
    handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl<C: Channel + 'static, F: ChannelFactory<C>> GenericBus<C, F>
where
    C::ConsumeReceipt: 'static,
{
    pub(crate) fn new(factory: F) -> Self {
        Self {
            factory,
            registry: Arc::new(ChannelRegistry::new()),
            subscriptions: Mutex::new(Vec::new()),
            invoker: Mutex::new(None),
            shared_reply_publisher: Mutex::new(None),
            cancel: CancellationToken::new(),
            handles: Mutex::new(Vec::new()),
        }
    }
}

impl<C: Channel + 'static, F: ChannelFactory<C> + 'static> Bus for GenericBus<C, F>
where
    C::ConsumeReceipt: 'static,
{
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        msg: Message,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let ch = self
                .registry
                .lookup(channel)
                .ok_or_else(|| anyhow::anyhow!("channel '{channel}' not found"))?;
            ch.publish(msg).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(())
        })
    }

    fn add_subscription(&self, config: SubscriptionConfig) {
        self.subscriptions.lock().unwrap().push(config);
    }

    fn set_invoker(&self, invoker: Arc<dyn ComponentInvoker>) {
        *self.invoker.lock().unwrap() = Some(invoker);
    }

    fn set_reply_publisher(&self, publisher: Arc<dyn ReplyPublisher>) {
        *self.shared_reply_publisher.lock().unwrap() = Some(publisher);
    }

    fn start(&self) -> Result<()> {
        let invoker = self
            .invoker
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Bus: invoker not set before start()"))?;

        // Compose the shared reply publisher with this bus's own registry.
        let shared = self.shared_reply_publisher.lock().unwrap().clone();
        let bus_registry = Arc::clone(&self.registry) as Arc<dyn ReplyPublisher>;
        let reply_publisher: Arc<dyn ReplyPublisher> = match shared {
            Some(shared) => Arc::new(CompositeReplyPublisher {
                ephemeral: shared,
                managed: bus_registry,
            }),
            None => bus_registry,
        };
        let subscriptions: Vec<_> = self.subscriptions.lock().unwrap().drain(..).collect();

        for sub in subscriptions {
            let channel = self.registry.lookup(&sub.channel_name).unwrap_or_else(|| {
                let ch = self.factory.create(&sub.channel_name);
                self.registry.register(&sub.channel_name, Arc::clone(&ch));
                ch
            });
            self.factory.init(&channel, &sub.component_name);

            let activator = Activator::new(
                Arc::clone(&invoker),
                &sub.component_name,
                None,
                Some(Arc::clone(&reply_publisher)),
            )
            .map_err(|e| anyhow::anyhow!(e))?;

            let dispatcher = Dispatcher::new(
                channel,
                sub.component_name.clone(),
                Arc::new(activator),
                1,
                self.cancel.child_token(),
            );

            let handle = tokio::spawn(async move {
                dispatcher.run().await;
            });
            self.handles.lock().unwrap().push(handle);
        }

        Ok(())
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        for channel in self.registry.list() {
            self.factory.close(&channel);
        }

        Box::pin(async {
            let mut handles: Vec<_> = self.handles.lock().unwrap().drain(..).collect();

            let graceful = async {
                for handle in handles.iter_mut() {
                    let _ = (&mut *handle).await;
                }
            };

            if tokio::time::timeout(Duration::from_secs(10), graceful)
                .await
                .is_err()
            {
                tracing::warn!("graceful shutdown timed out, cancelling dispatchers");
                self.cancel.cancel();
                for handle in handles {
                    let _ = handle.await;
                }
            }
        })
    }
}

// Factory for in-memory channels.
pub(crate) struct LocalChannelFactory;

impl ChannelFactory<super::channel::LocalChannel> for LocalChannelFactory {
    fn create(&self, _name: &str) -> Arc<super::channel::LocalChannel> {
        Arc::new(super::channel::LocalChannel::with_defaults())
    }

    fn init(&self, channel: &super::channel::LocalChannel, group: &str) {
        channel.init_group(group);
    }

    fn close(&self, channel: &super::channel::LocalChannel) {
        channel.close();
    }
}

pub(crate) type LocalBus = GenericBus<super::channel::LocalChannel, LocalChannelFactory>;

// Composes ephemeral reply channels with bus-managed channels into a single
// `ReplyPublisher`.
struct CompositeReplyPublisher {
    ephemeral: Arc<dyn ReplyPublisher>,
    managed: Arc<dyn ReplyPublisher>,
}

impl ReplyPublisher for CompositeReplyPublisher {
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        msg: Message,
    ) -> Pin<Box<dyn Future<Output = Result<(), super::channel::PublishError>> + Send + 'a>> {
        Box::pin(async move {
            match self.ephemeral.publish(channel, msg.clone()).await {
                Ok(()) => Ok(()),
                Err(super::channel::PublishError::Closed(_)) => {
                    self.managed.publish(channel, msg).await
                }
                Err(e) => Err(e),
            }
        })
    }
}
