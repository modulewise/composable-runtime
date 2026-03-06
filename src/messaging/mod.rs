//! Messaging for Wasm components.
//!
//! Provides message, channel, dispatcher, and activator.

mod activator;
mod channel;
mod dispatcher;
mod message;

pub(crate) use activator::{Activator, Handler, Mapper};
pub(crate) use channel::{
    Channel, ChannelRegistry, ConsumeError, ConsumeReceipt, LocalChannel, Overflow, PublishError,
    PublishReceipt, ReceiptError,
};
pub(crate) use dispatcher::Dispatcher;
pub(crate) use message::{
    FromHeaderValue, HeaderValue, Message, MessageBuilder, MessageHeaders, header,
};
