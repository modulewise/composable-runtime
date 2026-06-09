use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use uuid::Uuid;

use crate::context::PROPAGATION_CONTEXT;
use crate::types::PROPAGATED_HEADERS;

/// Return type for [`MessagePublisher::publish_request`].
pub type ReplyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ReturnAddress>>> + Send + 'a>>;

/// Header value types.
#[derive(Debug, Clone, PartialEq)]
pub enum HeaderValue {
    String(String),
    Integer(i64),
    Bool(bool),
}

impl std::fmt::Display for HeaderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderValue::String(s) => write!(f, "{s}"),
            HeaderValue::Integer(n) => write!(f, "{n}"),
            HeaderValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl From<String> for HeaderValue {
    fn from(s: String) -> Self {
        HeaderValue::String(s)
    }
}

impl From<&str> for HeaderValue {
    fn from(s: &str) -> Self {
        HeaderValue::String(s.to_string())
    }
}

impl From<i64> for HeaderValue {
    fn from(n: i64) -> Self {
        HeaderValue::Integer(n)
    }
}

impl From<bool> for HeaderValue {
    fn from(b: bool) -> Self {
        HeaderValue::Bool(b)
    }
}

/// Trait for extracting typed values from a [`HeaderValue`].
///
/// Enables `message.headers().get::<&str>("key")` with type inference.
pub trait FromHeaderValue<'a>: Sized {
    fn from_header_value(value: &'a HeaderValue) -> Option<Self>;
}

impl<'a> FromHeaderValue<'a> for &'a str {
    fn from_header_value(value: &'a HeaderValue) -> Option<Self> {
        match value {
            HeaderValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

impl<'a> FromHeaderValue<'a> for &'a HeaderValue {
    fn from_header_value(value: &'a HeaderValue) -> Option<Self> {
        Some(value)
    }
}

impl FromHeaderValue<'_> for i64 {
    fn from_header_value(value: &HeaderValue) -> Option<Self> {
        match value {
            HeaderValue::Integer(n) => Some(*n),
            _ => None,
        }
    }
}

impl FromHeaderValue<'_> for u64 {
    fn from_header_value(value: &HeaderValue) -> Option<Self> {
        match value {
            HeaderValue::Integer(n) => (*n).try_into().ok(),
            _ => None,
        }
    }
}

impl FromHeaderValue<'_> for bool {
    fn from_header_value(value: &HeaderValue) -> Option<Self> {
        match value {
            HeaderValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Message headers backed by a uniform key-value map.
///
/// Well-known headers have typed accessor methods. All headers (well-known and
/// custom) are stored in the same map and accessible via the generic
/// [`get`](MessageHeaders::get) method.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageHeaders {
    map: HashMap<String, HeaderValue>,
}

impl MessageHeaders {
    /// Unique message identifier.
    pub const ID: &str = "id";
    /// Message creation time (Unix epoch milliseconds).
    pub const TIMESTAMP: &str = "timestamp";
    /// Message time-to-live in milliseconds.
    pub const TTL: &str = "ttl";
    /// MIME type of the body.
    pub const CONTENT_TYPE: &str = "content-type";
    /// Return address for replies.
    pub const REPLY_TO: &str = "reply-to";
    /// Correlation identifier linking related messages.
    pub const CORRELATION_ID: &str = "correlation-id";

    /// Unique message identifier. Always present on a constructed [`Message`].
    pub fn id(&self) -> &str {
        match self.map.get(MessageHeaders::ID) {
            Some(HeaderValue::String(s)) => s.as_str(),
            _ => unreachable!("MessageBuilder guarantees id is present"),
        }
    }

    /// Message creation time (Unix epoch milliseconds). Always present on a
    /// constructed [`Message`].
    pub fn timestamp(&self) -> u64 {
        match self.map.get(MessageHeaders::TIMESTAMP) {
            Some(HeaderValue::Integer(n)) => *n as u64,
            _ => unreachable!("MessageBuilder guarantees timestamp is present"),
        }
    }

    /// Time-to-live in milliseconds.
    pub fn ttl(&self) -> Option<u64> {
        self.get(MessageHeaders::TTL)
    }

    /// MIME type of the body.
    pub fn content_type(&self) -> Option<&str> {
        self.get(MessageHeaders::CONTENT_TYPE)
    }

    /// Return address for replies.
    pub fn reply_to(&self) -> Option<&str> {
        self.get(MessageHeaders::REPLY_TO)
    }

    /// Links related messages across a conversation or task.
    pub fn correlation_id(&self) -> Option<&str> {
        self.get(MessageHeaders::CORRELATION_ID)
    }

    /// Get a header value by key.
    ///
    /// Returns `None` if the key is absent or the value is a different type.
    ///
    /// ```ignore
    /// let ct: Option<&str> = headers.get("content-type");
    /// let ttl: Option<u64> = headers.get("ttl");
    /// let custom: Option<bool> = headers.get("x-flag");
    /// let raw: Option<&HeaderValue> = headers.get("x-anything");
    /// ```
    pub fn get<'a, T: FromHeaderValue<'a>>(&'a self, key: &str) -> Option<T> {
        self.map.get(key).and_then(T::from_header_value)
    }

    /// Iterate over all headers as key-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &HeaderValue)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of headers.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the headers map is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// The internal message type.
///
/// Body is opaque bytes with content-type.
/// Only constructable via [`MessageBuilder`].
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    headers: MessageHeaders,
    body: Vec<u8>,
}

impl Message {
    /// Message headers.
    pub fn headers(&self) -> &MessageHeaders {
        &self.headers
    }

    /// Message body as raw bytes.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

/// Builder for constructing a [`Message`].
///
/// Generates `id` and `timestamp` if not already provided in the headers.
pub struct MessageBuilder {
    headers: HashMap<String, HeaderValue>,
    body: Vec<u8>,
}

impl MessageBuilder {
    /// Create a builder with the given body.
    pub fn new(body: Vec<u8>) -> Self {
        Self {
            headers: HashMap::new(),
            body,
        }
    }

    /// Create a builder pre-populated from an existing [`Message`].
    /// Subsequent [`header`](Self::header) calls override the inherited values.
    pub fn from_message(message: Message) -> Self {
        Self {
            headers: message.headers.map,
            body: message.body,
        }
    }

    /// Set a header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<HeaderValue>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Merge user-provided headers into the builder.
    pub fn headers(mut self, headers: HashMap<String, HeaderValue>) -> Self {
        self.headers.extend(headers);
        self
    }

    /// Build the [`Message`].
    ///
    /// If the calling task has a [`PropagationContext`] set, propagation
    /// headers (e.g. `traceparent`) are merged in unless the caller has
    /// already provided them (caller-supplied headers take precedence).
    pub fn build(mut self) -> Message {
        if !self.headers.contains_key(MessageHeaders::ID) {
            self.headers.insert(
                MessageHeaders::ID.to_string(),
                HeaderValue::String(Uuid::new_v4().to_string()),
            );
        }

        if !self.headers.contains_key(MessageHeaders::TIMESTAMP) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            self.headers.insert(
                MessageHeaders::TIMESTAMP.to_string(),
                HeaderValue::Integer(now),
            );
        }

        if let Some(entries) = PROPAGATION_CONTEXT
            .try_with(|ctx| ctx.as_ref().map(|c| c.entries.clone()))
            .ok()
            .flatten()
        {
            for key in PROPAGATED_HEADERS {
                if !self.headers.contains_key(*key)
                    && let Some(val) = entries.get(*key)
                {
                    self.headers
                        .insert((*key).to_string(), HeaderValue::String(val.clone()));
                }
            }
        }

        Message {
            headers: MessageHeaders { map: self.headers },
            body: self.body,
        }
    }
}

/// Publish messages to channels by name.
pub trait MessagePublisher: Send + Sync {
    /// Publish a message to the named channel.
    fn publish<'a>(
        &'a self,
        channel: &'a str,
        message: Message,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Publish a request message and use the return address to await the reply.
    fn publish_request<'a>(&'a self, channel: &'a str, message: Message) -> ReplyFuture<'a>;
}

/// Return address for a request-reply exchange.
pub trait ReturnAddress: Send {
    /// The ephemeral channel name placed in the outgoing `reply-to` header.
    fn channel(&self) -> &str;

    /// Await the reply. Cleans up the ephemeral channel after receiving.
    fn take(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<Message>> + Send>>;
}

/// One entry in a surface-level header-propagation list.
///
/// Parsed from a config string. `"foo"` means both source and target have the
/// same name (`foo`). `"foo as bar"` is a rename (source `foo`, target `bar`).
/// Whitespace around the names and the `as` keyword is tolerated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropagatedHeader {
    source: String,
    target: Option<String>,
}

impl PropagatedHeader {
    /// Parse a propagate entry from its config-string form.
    ///
    /// Accepts `"<source>"` or `"<source> as <target>"` (rename).
    /// Returns an error for empty source, empty target, or malformed input.
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        if s.trim().is_empty() {
            return Err("propagate entry is empty".to_string());
        }
        if let Some((src, tgt)) = s.split_once(" as ") {
            let source = src.trim().to_string();
            let target = tgt.trim().to_string();
            if source.is_empty() {
                return Err(format!("propagate entry '{s}' has empty source"));
            }
            if target.is_empty() {
                return Err(format!("propagate entry '{s}' has empty target"));
            }
            Ok(Self {
                source,
                target: Some(target),
            })
        } else {
            Ok(Self {
                source: s.trim().to_string(),
                target: None,
            })
        }
    }

    /// The source name (the side being read from).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The target name. Equals `source` when no rename was specified.
    pub fn target(&self) -> &str {
        self.target.as_deref().unwrap_or(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_generates_id_and_timestamp() {
        let msg = MessageBuilder::new(b"hello".to_vec()).build();

        assert!(!msg.headers().id().is_empty());
        assert!(msg.headers().timestamp() > 0);
        assert_eq!(msg.body(), b"hello");
    }

    #[test]
    fn builder_preserves_user_provided_id() {
        let msg = MessageBuilder::new(b"hello".to_vec())
            .header(MessageHeaders::ID, "custom-id-1")
            .build();

        assert_eq!(msg.headers().id(), "custom-id-1");
    }

    #[test]
    fn builder_with_well_known_headers() {
        let msg = MessageBuilder::new(b"{}".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "application/json")
            .header(MessageHeaders::CORRELATION_ID, "corr-1")
            .header(MessageHeaders::TTL, 5000_i64)
            .header(MessageHeaders::REPLY_TO, "reply-chan")
            .build();

        assert_eq!(msg.headers().content_type(), Some("application/json"));
        assert_eq!(msg.headers().correlation_id(), Some("corr-1"));
        assert_eq!(msg.headers().ttl(), Some(5000));
        assert_eq!(msg.headers().reply_to(), Some("reply-chan"));
    }

    #[test]
    fn builder_with_extension_headers() {
        let msg = MessageBuilder::new(b"{}".to_vec())
            .header("traceparent", "00-abc-def-01")
            .header("x-flag", true)
            .header("x-count", 42_i64)
            .build();

        let tp: Option<&str> = msg.headers().get("traceparent");
        assert_eq!(tp, Some("00-abc-def-01"));

        let flag: Option<bool> = msg.headers().get("x-flag");
        assert_eq!(flag, Some(true));

        let count: Option<i64> = msg.headers().get("x-count");
        assert_eq!(count, Some(42));
    }

    #[test]
    fn get_returns_none_for_missing_key() {
        let msg = MessageBuilder::new(b"".to_vec()).build();

        let missing: Option<&str> = msg.headers().get("nonexistent");
        assert_eq!(missing, None);
    }

    #[test]
    fn get_returns_none_for_type_mismatch() {
        let msg = MessageBuilder::new(b"".to_vec())
            .header("x-count", 42_i64)
            .build();

        // x-count is an Integer, requesting &str returns None
        let wrong_type: Option<&str> = msg.headers().get("x-count");
        assert_eq!(wrong_type, None);
    }

    #[test]
    fn get_raw_header_value() {
        let msg = MessageBuilder::new(b"".to_vec())
            .header("x-count", 42_i64)
            .build();

        let raw: Option<&HeaderValue> = msg.headers().get("x-count");
        assert_eq!(raw, Some(&HeaderValue::Integer(42)));
    }

    #[test]
    fn builder_with_hashmap_headers() {
        let mut user_headers = HashMap::new();
        user_headers.insert(
            MessageHeaders::CONTENT_TYPE.to_string(),
            HeaderValue::from("text/plain"),
        );
        user_headers.insert("x-custom".to_string(), HeaderValue::from("value"));

        let msg = MessageBuilder::new(b"hello".to_vec())
            .headers(user_headers)
            .build();

        assert_eq!(msg.headers().content_type(), Some("text/plain"));
        let custom: Option<&str> = msg.headers().get("x-custom");
        assert_eq!(custom, Some("value"));
    }

    #[test]
    fn headers_iter() {
        let msg = MessageBuilder::new(b"".to_vec())
            .header(MessageHeaders::CONTENT_TYPE, "text/plain")
            .build();

        // At minimum: id, timestamp, content-type
        assert!(msg.headers().len() >= 3);

        let keys: Vec<&str> = msg.headers().iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&MessageHeaders::ID));
        assert!(keys.contains(&MessageHeaders::TIMESTAMP));
        assert!(keys.contains(&MessageHeaders::CONTENT_TYPE));
    }

    #[test]
    fn propagated_header_identity() {
        let p = PropagatedHeader::parse("X-Request-Id").unwrap();
        assert_eq!(p.source(), "X-Request-Id");
        assert_eq!(p.target(), "X-Request-Id");
    }

    #[test]
    fn propagated_header_rename() {
        let p = PropagatedHeader::parse("X-Request-Id as request-id").unwrap();
        assert_eq!(p.source(), "X-Request-Id");
        assert_eq!(p.target(), "request-id");
    }

    #[test]
    fn propagated_header_tolerates_whitespace() {
        let p = PropagatedHeader::parse("  foo  as  bar  ").unwrap();
        assert_eq!(p.source(), "foo");
        assert_eq!(p.target(), "bar");
    }

    #[test]
    fn propagated_header_empty_input_errors() {
        assert!(PropagatedHeader::parse("").is_err());
        assert!(PropagatedHeader::parse("   ").is_err());
    }

    #[test]
    fn propagated_header_empty_source_or_target_errors() {
        assert!(PropagatedHeader::parse(" as bar").is_err());
        assert!(PropagatedHeader::parse("foo as ").is_err());
    }

    #[test]
    fn propagated_header_as_in_name_not_treated_as_separator() {
        // `as` only separates when surrounded by spaces. A name containing
        // `as` substring without surrounding spaces should not split.
        let p = PropagatedHeader::parse("classification").unwrap();
        assert_eq!(p.source(), "classification");
        assert_eq!(p.target(), "classification");
    }
}
