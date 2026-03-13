use std::sync::Arc;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::activator::Handler;
use super::channel::{Channel, ConsumeError, ConsumeReceipt};

/// Per-subscription consumer that connects a channel to a handler.
///
/// Consumes messages from a channel using a group name and dispatches
/// each to the handler. Concurrency controls how many handler invocations
/// run in parallel via a `JoinSet`.
pub struct Dispatcher<C: Channel, H: Handler> {
    channel: Arc<C>,
    group: String,
    handler: Arc<H>,
    concurrency: usize,
    cancel: CancellationToken,
}

impl<C: Channel + 'static, H: Handler + 'static> Dispatcher<C, H>
where
    C::ConsumeReceipt: 'static,
{
    pub fn new(
        channel: Arc<C>,
        group: String,
        handler: Arc<H>,
        concurrency: usize,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            channel,
            group,
            handler,
            concurrency,
            cancel,
        }
    }

    /// Run the dispatch loop. Returns when cancelled or channel is closed.
    pub async fn run(&self) {
        let mut tasks = JoinSet::new();

        loop {
            // Backpressure: wait for capacity before consuming.
            if tasks.len() >= self.concurrency {
                tasks.join_next().await;
            }

            tokio::select! {
                biased;
                result = self.channel.consume(&self.group) => {
                    match result {
                        Ok((msg, receipt)) => {
                            self.dispatch_message(&mut tasks, msg, receipt);
                        }
                        Err(ConsumeError::Timeout(_)) => {
                            tracing::debug!(group = %self.group, "no message, looping");
                            continue;
                        }
                        Err(ConsumeError::Closed(ref msg)) => {
                            tracing::debug!(group = %self.group, reason = %msg, "channel closed, exiting");
                            break;
                        }
                    }
                }
                _ = self.cancel.cancelled() => break,
            }
        }

        // Drain remaining in-flight tasks.
        while tasks.join_next().await.is_some() {}
    }

    fn dispatch_message(
        &self,
        tasks: &mut JoinSet<()>,
        msg: super::Message,
        receipt: C::ConsumeReceipt,
    ) {
        let handler = Arc::clone(&self.handler);
        tasks.spawn(async move {
            match handler.handle(msg).await {
                Ok(()) => {
                    if let Err(e) = receipt.ack().await {
                        tracing::error!(error = %e, "ack failed");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "handler error");
                    if let Err(e) = receipt.nack().await {
                        tracing::error!(error = %e, "nack failed");
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::Semaphore;

    use super::*;
    use crate::messaging::channel::{
        ConsumeError, LocalChannel, Overflow, PublishError, ReceiptError,
    };
    use crate::messaging::message::{Message, MessageBuilder};

    fn init_tracing() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }

    // Tracks handle() invocations with a watch channel for sleep-free waiting.
    struct HandleCounter {
        tx: tokio::sync::watch::Sender<usize>,
        rx: tokio::sync::watch::Receiver<usize>,
    }

    impl HandleCounter {
        fn new() -> Self {
            let (tx, rx) = tokio::sync::watch::channel(0);
            Self { tx, rx }
        }

        fn increment(&self) {
            self.tx.send_modify(|c| *c += 1);
        }

        // Wait until at least `n` handle() calls have completed.
        // Panics if not reached within 5 seconds.
        async fn wait_for(&self, n: usize) {
            let mut rx = self.rx.clone();
            tokio::time::timeout(Duration::from_secs(5), rx.wait_for(|&c| c >= n))
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {n} handle() calls"))
                .unwrap();
        }
    }

    // Stub handler that captures received messages.
    struct StubHandler {
        received: tokio::sync::Mutex<Vec<Message>>,
        counter: HandleCounter,
    }

    impl StubHandler {
        fn new() -> Self {
            Self {
                received: tokio::sync::Mutex::new(Vec::new()),
                counter: HandleCounter::new(),
            }
        }

        async fn received(&self) -> Vec<Message> {
            self.received.lock().await.clone()
        }
    }

    impl Handler for StubHandler {
        async fn handle(&self, msg: Message) -> Result<(), String> {
            self.received.lock().await.push(msg);
            self.counter.increment();
            Ok(())
        }
    }

    // Handler that always returns an error.
    struct ErrorHandler {
        counter: HandleCounter,
    }

    impl ErrorHandler {
        fn new() -> Self {
            Self {
                counter: HandleCounter::new(),
            }
        }
    }

    impl Handler for ErrorHandler {
        async fn handle(&self, _msg: Message) -> Result<(), String> {
            self.counter.increment();
            Err("test error".to_string())
        }
    }

    // Handler that blocks on a semaphore, for testing concurrency.
    struct BlockingHandler {
        semaphore: Arc<Semaphore>,
        active: AtomicUsize,
        max_active: AtomicUsize,
        counter: HandleCounter,
    }

    impl BlockingHandler {
        fn new(semaphore: Arc<Semaphore>) -> Self {
            Self {
                semaphore,
                active: AtomicUsize::new(0),
                max_active: AtomicUsize::new(0),
                counter: HandleCounter::new(),
            }
        }
    }

    impl Handler for BlockingHandler {
        async fn handle(&self, _msg: Message) -> Result<(), String> {
            let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(current, Ordering::SeqCst);
            self.counter.increment();
            let _permit = self.semaphore.acquire().await.unwrap();
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // Channel wrapper that tracks ack/nack calls via a custom receipt.
    struct TrackingChannel {
        inner: LocalChannel,
        acks: Arc<AtomicUsize>,
        nacks: Arc<AtomicUsize>,
    }

    impl TrackingChannel {
        fn new() -> Self {
            Self {
                inner: LocalChannel::new(256, Overflow::Block, -1, 50),
                acks: Arc::new(AtomicUsize::new(0)),
                nacks: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    struct TrackingReceipt {
        acks: Arc<AtomicUsize>,
        nacks: Arc<AtomicUsize>,
    }

    impl ConsumeReceipt for TrackingReceipt {
        async fn ack(self) -> Result<(), ReceiptError> {
            self.acks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn nack(self) -> Result<(), ReceiptError> {
            self.nacks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    impl Channel for TrackingChannel {
        type ConsumeReceipt = TrackingReceipt;
        type PublishReceipt = ();

        async fn publish(&self, msg: Message) -> Result<(), PublishError> {
            self.inner.publish(msg).await
        }

        async fn consume(&self, group: &str) -> Result<(Message, TrackingReceipt), ConsumeError> {
            let (msg, _) = self.inner.consume(group).await?;
            let receipt = TrackingReceipt {
                acks: Arc::clone(&self.acks),
                nacks: Arc::clone(&self.nacks),
            };
            Ok((msg, receipt))
        }
    }

    #[tokio::test]
    async fn ack_called_on_handler_success() {
        let channel = Arc::new(TrackingChannel::new());
        let handler = Arc::new(StubHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"hello".to_vec()).build();
        channel.publish(msg).await.unwrap();

        handler.counter.wait_for(1).await;
        cancel.cancel();
        dispatch_handle.await.unwrap();

        assert_eq!(channel.acks.load(Ordering::SeqCst), 1);
        assert_eq!(channel.nacks.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nack_called_on_handler_error() {
        let channel = Arc::new(TrackingChannel::new());
        let handler = Arc::new(ErrorHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"fail".to_vec()).build();
        channel.publish(msg).await.unwrap();

        handler.counter.wait_for(1).await;
        cancel.cancel();
        dispatch_handle.await.unwrap();

        assert_eq!(channel.acks.load(Ordering::SeqCst), 0);
        assert_eq!(channel.nacks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn single_message_dispatched() {
        let channel = Arc::new(LocalChannel::new(256, Overflow::Block, -1, 50));
        let handler = Arc::new(StubHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        // Start consumer first so the group exists.
        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        let msg = MessageBuilder::new(b"hello".to_vec()).build();
        channel.publish(msg).await.unwrap();

        handler.counter.wait_for(1).await;
        cancel.cancel();
        dispatch_handle.await.unwrap();

        let received = handler.received().await;
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].body(), b"hello");
    }

    #[tokio::test]
    async fn concurrency_limit_respected() {
        let channel = Arc::new(LocalChannel::new(256, Overflow::Block, -1, 50));
        let semaphore = Arc::new(Semaphore::new(0));
        let handler = Arc::new(BlockingHandler::new(Arc::clone(&semaphore)));
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        // Publish 3 messages.
        for i in 0..3 {
            let msg = MessageBuilder::new(format!("msg{i}").into_bytes()).build();
            channel.publish(msg).await.unwrap();
        }

        // Wait for the first handler to enter (increments counter before blocking).
        handler.counter.wait_for(1).await;

        // With concurrency=1, only 1 should be active.
        assert_eq!(handler.max_active.load(Ordering::SeqCst), 1);

        // Release all and wait for completion.
        semaphore.add_permits(3);
        handler.counter.wait_for(3).await;

        assert_eq!(handler.active.load(Ordering::SeqCst), 0);
        assert_eq!(handler.max_active.load(Ordering::SeqCst), 1);

        cancel.cancel();
        dispatch_handle.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_exits_run() {
        let channel = Arc::new(LocalChannel::new(256, Overflow::Block, -1, -1));
        let handler = Arc::new(StubHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        cancel.cancel();
        // run() should return promptly.
        tokio::time::timeout(Duration::from_secs(1), dispatch_handle)
            .await
            .expect("dispatcher did not exit within timeout")
            .unwrap();
    }

    #[tokio::test]
    async fn timeout_loops_silently() {
        init_tracing();
        let channel = Arc::new(LocalChannel::new(256, Overflow::Block, -1, 50));
        let handler = Arc::new(StubHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });

        // Let it loop a few times on timeout with no messages.
        tokio::time::sleep(Duration::from_millis(200)).await;

        cancel.cancel();
        dispatch_handle.await.unwrap();

        // No messages should have been received.
        assert!(handler.received().await.is_empty());
    }

    #[tokio::test]
    async fn handler_error_continues_dispatching() {
        let channel = Arc::new(LocalChannel::new(256, Overflow::Block, -1, 50));
        let handler = Arc::new(ErrorHandler::new());
        let cancel = CancellationToken::new();

        let dispatcher = Dispatcher::new(
            Arc::clone(&channel),
            "test".to_string(),
            Arc::clone(&handler),
            1,
            cancel.clone(),
        );

        let dispatch_handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::task::yield_now().await;

        // Publish two messages. Both should be consumed even though handler errors.
        let msg1 = MessageBuilder::new(b"first".to_vec()).build();
        let msg2 = MessageBuilder::new(b"second".to_vec()).build();
        channel.publish(msg1).await.unwrap();
        channel.publish(msg2).await.unwrap();

        // Wait for both to be handled (errors don't stop the loop).
        handler.counter.wait_for(2).await;

        cancel.cancel();
        dispatch_handle.await.unwrap();
    }
}
