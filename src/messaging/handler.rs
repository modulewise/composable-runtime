use std::future::Future;

use super::Message;

/// Handler for incoming messages.
///
/// Implemented by the activator (for domain components) and by the native
/// handler adapter (for components exporting the WIT `handler` interface).
/// The dispatcher delivers messages through this trait without knowing
/// which kind of handler it is.
pub trait Handler: Send + Sync {
    fn handle(&self, msg: Message) -> impl Future<Output = Result<(), String>> + Send;
}
