//! Messaging for Wasm components.
//!
//! Provides the internal messaging primitives: message, channel, and dispatcher.

mod activator;
mod channel;
mod dispatcher;
mod message;

pub(crate) use activator::{Activator, Handler, Mapper};
pub(crate) use channel::{
    Channel, ChannelRegistry, ConsumeError, LocalChannel, Overflow, PublishError,
};
pub(crate) use dispatcher::Dispatcher;
pub(crate) use message::{
    FromHeaderValue, HeaderValue, Message, MessageBuilder, MessageHeaders, header,
};
