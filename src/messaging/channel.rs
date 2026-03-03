use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time;

use super::Message;

// Default channel capacity.
const DEFAULT_CAPACITY: usize = 256;

// Default timeout in milliseconds for publish and consume.
// Callers may wrap with a shorter timeout.
const DEFAULT_TIMEOUT_MS: i64 = 30_000;

/// Publish failed.
#[derive(Debug)]
pub enum PublishError {
    /// Channel is closed.
    Closed(String),
    /// Channel at capacity and publish timeout was 0 (immediate rejection).
    Full(String),
    /// Channel at capacity after waiting up to publish timeout.
    Timeout(String),
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublishError::Closed(msg) => write!(f, "channel closed: {msg}"),
            PublishError::Full(msg) => write!(f, "channel full: {msg}"),
            PublishError::Timeout(msg) => write!(f, "publish timeout: {msg}"),
        }
    }
}

impl std::error::Error for PublishError {}

/// Consume failed.
#[derive(Debug)]
pub enum ConsumeError {
    /// No message available within consume timeout.
    Timeout(String),
    /// Channel is closed.
    Closed(String),
}

impl std::fmt::Display for ConsumeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsumeError::Closed(msg) => write!(f, "channel closed: {msg}"),
            ConsumeError::Timeout(msg) => write!(f, "consume timeout: {msg}"),
        }
    }
}

impl std::error::Error for ConsumeError {}

/// Overflow behavior when the channel is at capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// Publisher blocks until space is available (or timeout/close).
    Block,
    /// Oldest message is dropped to make room.
    DropOldest,
}

/// Messaging channel.
///
/// Consumer-group semantics: consumers sharing a group name compete for
/// messages (point-to-point). Consumers with distinct group names each
/// independently receive every message (pub-sub).
pub trait Channel: Send + Sync {
    async fn publish(&self, msg: Message) -> Result<(), PublishError>;
    async fn consume(&self, group: &str) -> Result<Message, ConsumeError>;
}

/// In-memory channel backed by tokio primitives.
///
/// - `Overflow::Block`: one `mpsc` sender/receiver per consumer group.
///   Publisher clones and sends to each group. Blocks when any group is full.
/// - `Overflow::DropOldest`: `broadcast` channel. One shared sender, one
///   receiver per group. Oldest messages overwritten for slow consumers.
///
/// Channels start with no consumer groups. Publishing to a channel with no
/// groups returns `PublishError::Closed`. Groups are created on the
/// first `consume()` call with a given group name. All groups only receive
/// messages published after the group is created.
pub struct LocalChannel {
    capacity: usize,
    publish_timeout_ms: i64,
    consume_timeout_ms: i64,
    inner: ChannelInner,
}

enum ChannelInner {
    Block {
        // Group name => sender. Used by publish to fan out to all groups.
        senders: Mutex<HashMap<String, mpsc::Sender<Message>>>,
        // Group name => receiver (behind async Mutex for competing consumers).
        receivers: Mutex<HashMap<String, Arc<tokio::sync::Mutex<mpsc::Receiver<Message>>>>>,
    },
    DropOldest {
        // Shared sender.
        sender: tokio::sync::broadcast::Sender<Message>,
        // Group name => receiver (behind async Mutex for competing consumers).
        receivers: Mutex<
            HashMap<String, Arc<tokio::sync::Mutex<tokio::sync::broadcast::Receiver<Message>>>>,
        >,
    },
}

impl LocalChannel {
    /// Create a new `LocalChannel` with the given configuration.
    pub fn new(
        capacity: usize,
        overflow: Overflow,
        publish_timeout_ms: i64,
        consume_timeout_ms: i64,
    ) -> Self {
        let inner = match overflow {
            Overflow::Block => ChannelInner::Block {
                senders: Mutex::new(HashMap::new()),
                receivers: Mutex::new(HashMap::new()),
            },
            Overflow::DropOldest => {
                let (sender, _) = tokio::sync::broadcast::channel(capacity);
                ChannelInner::DropOldest {
                    sender,
                    receivers: Mutex::new(HashMap::new()),
                }
            }
        };

        Self {
            capacity,
            publish_timeout_ms,
            consume_timeout_ms,
            inner,
        }
    }

    /// Create a `LocalChannel` with default settings.
    pub fn with_defaults() -> Self {
        Self::new(
            DEFAULT_CAPACITY,
            Overflow::Block,
            DEFAULT_TIMEOUT_MS,
            DEFAULT_TIMEOUT_MS,
        )
    }
}

impl Channel for LocalChannel {
    async fn publish(&self, msg: Message) -> Result<(), PublishError> {
        match &self.inner {
            ChannelInner::Block {
                senders, receivers, ..
            } => {
                let group_senders: Vec<mpsc::Sender<Message>> = {
                    let receiver_map = receivers.lock().unwrap();
                    if receiver_map.is_empty() {
                        return Err(PublishError::Closed("no active receivers".to_string()));
                    }
                    drop(receiver_map);
                    let map = senders.lock().unwrap();
                    map.values().cloned().collect()
                };

                for sender in &group_senders {
                    match self.publish_timeout_ms {
                        t if t < 0 => {
                            sender.send(msg.clone()).await.map_err(|_| {
                                PublishError::Closed("no active receivers".to_string())
                            })?;
                        }
                        0 => {
                            sender.try_send(msg.clone()).map_err(|e| match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    PublishError::Full("channel at capacity".to_string())
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    PublishError::Closed("no active receivers".to_string())
                                }
                            })?;
                        }
                        t => {
                            let timeout_dur = Duration::from_millis(t as u64);
                            match time::timeout(timeout_dur, sender.send(msg.clone())).await {
                                Ok(Ok(())) => {}
                                Ok(Err(_)) => {
                                    return Err(PublishError::Closed(
                                        "no active receivers".to_string(),
                                    ));
                                }
                                Err(_) => {
                                    return Err(PublishError::Timeout(format!(
                                        "no space after {t}ms"
                                    )));
                                }
                            }
                        }
                    }
                }

                Ok(())
            }
            ChannelInner::DropOldest { sender, receivers } => {
                {
                    let map = receivers.lock().unwrap();
                    if map.is_empty() {
                        return Err(PublishError::Closed("no active receivers".to_string()));
                    }
                }
                sender
                    .send(msg)
                    .map_err(|_| PublishError::Closed("no active receivers".to_string()))?;
                Ok(())
            }
        }
    }

    async fn consume(&self, group: &str) -> Result<Message, ConsumeError> {
        match &self.inner {
            ChannelInner::Block { senders, receivers } => {
                let receiver_mutex = {
                    let mut receiver_map = receivers.lock().unwrap();

                    if let Some(r) = receiver_map.get(group) {
                        Arc::clone(r)
                    } else {
                        let (tx, rx) = mpsc::channel(self.capacity);
                        let arc = Arc::new(tokio::sync::Mutex::new(rx));
                        receiver_map.insert(group.to_string(), Arc::clone(&arc));
                        drop(receiver_map);
                        let mut sender_map = senders.lock().unwrap();
                        sender_map.insert(group.to_string(), tx);
                        arc
                    }
                };

                let mut receiver = receiver_mutex.lock().await;
                match self.consume_timeout_ms {
                    t if t < 0 => receiver
                        .recv()
                        .await
                        .ok_or_else(|| ConsumeError::Closed("no active senders".to_string())),
                    0 => receiver.try_recv().map_err(|e| match e {
                        mpsc::error::TryRecvError::Empty => {
                            ConsumeError::Timeout("no message available".to_string())
                        }
                        mpsc::error::TryRecvError::Disconnected => {
                            ConsumeError::Closed("no active senders".to_string())
                        }
                    }),
                    t => {
                        let timeout_dur = Duration::from_millis(t as u64);
                        match time::timeout(timeout_dur, receiver.recv()).await {
                            Ok(Some(msg)) => Ok(msg),
                            Ok(None) => Err(ConsumeError::Closed("no active senders".to_string())),
                            Err(_) => Err(ConsumeError::Timeout(format!("no message after {t}ms"))),
                        }
                    }
                }
            }
            ChannelInner::DropOldest { sender, receivers } => {
                let receiver_mutex = {
                    let mut map = receivers.lock().unwrap();

                    if let Some(r) = map.get(group) {
                        Arc::clone(r)
                    } else {
                        let arc = Arc::new(tokio::sync::Mutex::new(sender.subscribe()));
                        map.insert(group.to_string(), Arc::clone(&arc));
                        arc
                    }
                };

                let mut receiver = receiver_mutex.lock().await;
                match self.consume_timeout_ms {
                    0 => match receiver.try_recv() {
                        Ok(msg) => Ok(msg),
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                            Err(ConsumeError::Timeout("no message available".to_string()))
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            tracing::warn!(
                                group,
                                skipped = n,
                                "consumer group lagged, skipped {n} messages"
                            );
                            Err(ConsumeError::Timeout("no message available".to_string()))
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            Err(ConsumeError::Closed("no active senders".to_string()))
                        }
                    },
                    t => {
                        let recv_fut = async {
                            loop {
                                match receiver.recv().await {
                                    Ok(msg) => return Ok(msg),
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!(
                                            group,
                                            skipped = n,
                                            "consumer group lagged, skipped {n} messages"
                                        );
                                        continue;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                        return Err(ConsumeError::Closed(
                                            "no active senders".to_string(),
                                        ));
                                    }
                                }
                            }
                        };

                        if t < 0 {
                            recv_fut.await
                        } else {
                            let timeout_dur = Duration::from_millis(t as u64);
                            match time::timeout(timeout_dur, recv_fut).await {
                                Ok(result) => result,
                                Err(_) => {
                                    Err(ConsumeError::Timeout(format!("no message after {t}ms")))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::messaging::MessageBuilder;

    #[tokio::test]
    async fn publish_no_consumers_returns_error() {
        let channel = LocalChannel::with_defaults();

        let msg = MessageBuilder::new(b"hello".to_vec()).build();
        let result = channel.publish(msg).await;
        assert!(matches!(result, Err(PublishError::Closed(_))));
    }

    #[tokio::test]
    async fn publish_no_consumers_drop_oldest_returns_error() {
        let channel = LocalChannel::new(256, Overflow::DropOldest, -1, -1);

        let msg = MessageBuilder::new(b"hello".to_vec()).build();
        let result = channel.publish(msg).await;
        assert!(matches!(result, Err(PublishError::Closed(_))));
    }

    #[tokio::test]
    async fn consume_then_publish() {
        let channel = Arc::new(LocalChannel::with_defaults());

        // Start consumer first. Creates the group.
        let consumer = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };

        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"hello".to_vec()).build();
        channel.publish(msg).await.unwrap();

        let received = consumer.await.unwrap().unwrap();
        assert_eq!(received.body(), b"hello");
    }

    #[tokio::test]
    async fn competing_consumers_same_group() {
        let channel = Arc::new(LocalChannel::with_defaults());

        // Start two competing consumers.
        let c1 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };
        let c2 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };

        tokio::task::yield_now().await;

        // Publish two messages.
        let msg1 = MessageBuilder::new(b"msg1".to_vec()).build();
        let msg2 = MessageBuilder::new(b"msg2".to_vec()).build();
        channel.publish(msg1).await.unwrap();
        channel.publish(msg2).await.unwrap();

        let r1 = c1.await.unwrap().unwrap();
        let r2 = c2.await.unwrap().unwrap();

        // Each consumer gets exactly one unique message.
        assert_ne!(r1.body(), r2.body());
    }

    #[tokio::test]
    async fn independent_groups_each_get_message() {
        let channel = Arc::new(LocalChannel::with_defaults());

        // Start two consumers on different groups.
        let c1 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-a").await })
        };
        let c2 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-b").await })
        };

        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"broadcast".to_vec()).build();
        channel.publish(msg).await.unwrap();

        let r1 = c1.await.unwrap().unwrap();
        let r2 = c2.await.unwrap().unwrap();

        // Both groups receive the same message.
        assert_eq!(r1.body(), b"broadcast");
        assert_eq!(r2.body(), b"broadcast");
    }

    #[tokio::test]
    async fn publish_timeout_zero_returns_full() {
        let channel = Arc::new(LocalChannel::new(1, Overflow::Block, 0, -1));

        // Create the group first.
        let consumer = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };

        tokio::task::yield_now().await;

        // Fill the single-capacity group.
        let msg1 = MessageBuilder::new(b"first".to_vec()).build();
        channel.publish(msg1).await.unwrap();

        // Second publish should fail immediately with Full.
        let msg2 = MessageBuilder::new(b"second".to_vec()).build();
        let result = channel.publish(msg2).await;
        assert!(matches!(result, Err(PublishError::Full(_))));

        // Drain.
        let _ = consumer.await;
    }

    #[tokio::test]
    async fn publish_timeout_expires() {
        let channel = Arc::new(LocalChannel::new(1, Overflow::Block, 50, -1));

        // Create the group by starting a consume, then let it receive msg1.
        let c1 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("test").await })
        };

        tokio::task::yield_now().await;

        // Fill the channel.
        let msg1 = MessageBuilder::new(b"first".to_vec()).build();
        channel.publish(msg1).await.unwrap();

        // c1 will drain msg1, but no other pending consume call.
        let _ = c1.await;

        // Re-fill.
        let msg2 = MessageBuilder::new(b"second".to_vec()).build();
        channel.publish(msg2).await.unwrap();

        // Should timeout. No consumer is draining.
        let msg3 = MessageBuilder::new(b"third".to_vec()).build();
        let result = channel.publish(msg3).await;
        assert!(matches!(result, Err(PublishError::Timeout(_))));

        // Drain.
        let _ = channel.consume("test").await;
    }

    #[tokio::test]
    async fn consume_timeout_expires() {
        let channel = LocalChannel::new(256, Overflow::Block, -1, 50);

        // Group created on first consume call. No messages.
        let result = channel.consume("test").await;
        assert!(matches!(result, Err(ConsumeError::Timeout(_))));
    }

    #[tokio::test]
    async fn drop_oldest_independent_groups() {
        let channel = Arc::new(LocalChannel::new(256, Overflow::DropOldest, -1, -1));

        // Create two groups.
        let c1 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-a").await })
        };
        let c2 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-b").await })
        };

        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"fanout".to_vec()).build();
        channel.publish(msg).await.unwrap();

        let r1 = c1.await.unwrap().unwrap();
        let r2 = c2.await.unwrap().unwrap();

        assert_eq!(r1.body(), b"fanout");
        assert_eq!(r2.body(), b"fanout");
    }

    #[tokio::test]
    async fn new_group_only_sees_messages_after_first_consume() {
        let channel = Arc::new(LocalChannel::with_defaults());

        // Create first group and publish a message to it.
        let c1 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-a").await })
        };

        tokio::task::yield_now().await;

        let msg1 = MessageBuilder::new(b"before".to_vec()).build();
        channel.publish(msg1).await.unwrap();

        let r1 = c1.await.unwrap().unwrap();
        assert_eq!(r1.body(), b"before");

        // Now create a second group. It should NOT see the previous message.
        let c2 = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-b").await })
        };

        tokio::task::yield_now().await;

        // Publish a new message. Both groups should see it.
        let c1_again = {
            let ch = channel.clone();
            tokio::spawn(async move { ch.consume("group-a").await })
        };

        let msg2 = MessageBuilder::new(b"after".to_vec()).build();
        channel.publish(msg2).await.unwrap();

        let r2 = c2.await.unwrap().unwrap();
        let r1_again = c1_again.await.unwrap().unwrap();
        assert_eq!(r2.body(), b"after");
        assert_eq!(r1_again.body(), b"after");
    }
}
