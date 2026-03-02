//! Messaging for Wasm components.
//!
//! Provides the internal messaging primitives: message, channel, and dispatcher.

mod handler;
mod message;

pub(crate) use handler::Handler;
pub(crate) use message::{
    FromHeaderValue, HeaderValue, Message, MessageBuilder, MessageHeaders, header,
};
