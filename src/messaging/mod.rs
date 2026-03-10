//! Messaging for Wasm components.
//!
//! Provides message, channel, dispatcher, and activator.

mod activator;
mod bus;
mod channel;
mod dispatcher;
mod message;
pub(crate) mod service;

pub use channel::Channel;
pub use message::{Message, MessageBuilder, header};

pub(crate) use service::MessagingService;
