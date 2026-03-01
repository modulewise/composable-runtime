//! Messaging for Wasm components.
//!
//! Provides the internal messaging primitives: message, channel, and dispatcher.

mod message;

pub(crate) use message::{
    FromHeaderValue, HeaderValue, Message, MessageBuilder, MessageHeaders, header,
};
