//! Context propagated across service boundaries.
//!
//! `PROPAGATION_CONTEXT` is a tokio task-local that carries key/value entries
//! (e.g. W3C traceparent, tracestate, baggage) across invocation and messaging
//! boundaries. Scoped by `ComponentInvoker` and transport entry points.
//! Readers should use `try_with` to handle the case where no scope is active.

use std::collections::HashMap;

/// Per-invocation context carrying propagated key/value entries.
pub struct PropagationContext {
    pub entries: HashMap<String, String>,
}

tokio::task_local! {
    pub static PROPAGATION_CONTEXT: Option<PropagationContext>;
}
