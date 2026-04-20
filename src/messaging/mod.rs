//! Messaging for Wasm components.
//!
//! Provides message, channel, dispatcher, and activator.

mod activator;
mod bus;
mod channel;
mod dispatcher;
mod reply;
pub(crate) mod service;

pub use channel::Channel;

pub(crate) use service::MessagingService;
