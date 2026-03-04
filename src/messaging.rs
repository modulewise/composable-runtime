//! Messaging for Wasm components.
//!
//! Provides the internal messaging primitives: message, channel, and dispatcher.

mod channel;
mod dispatcher;
mod handler;
mod message;

pub(crate) use channel::{Channel, ConsumeError, LocalChannel, Overflow, PublishError};
pub(crate) use dispatcher::Dispatcher;
pub(crate) use handler::Handler;
pub(crate) use message::{
    FromHeaderValue, HeaderValue, Message, MessageBuilder, MessageHeaders, header,
};
