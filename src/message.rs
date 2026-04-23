use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use uuid::Uuid;

/// Return type for [`MessagePublisher::publish_request`].
pub type ReplyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Box<dyn ReturnAddress>>> + Send + 'a>>;

/// Well-known header keys.
pub mod header {
    pub const ID: &str = "id";
    pub const TIMESTAMP: &str = "timestamp";
    pub const TTL: &str = "ttl";
    pub const CONTENT_TYPE: &str = "content-type";
    pub const REPLY_TO: &str = "reply-to";
    pub const CORRELATION_ID: &str = "correlation-id";
}

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
    /// Unique message identifier. Always present on a constructed [`Message`].
    pub fn id(&self) -> &str {
        match self.map.get(header::ID) {
            Some(HeaderValue::String(s)) => s.as_str(),
            _ => unreachable!("MessageBuilder guarantees id is present"),
        }
    }

    /// Message creation time (Unix epoch milliseconds). Always present on a
    /// constructed [`Message`].
    pub fn timestamp(&self) -> u64 {
        match self.map.get(header::TIMESTAMP) {
            Some(HeaderValue::Integer(n)) => *n as u64,
            _ => unreachable!("MessageBuilder guarantees timestamp is present"),
        }
    }

    /// Time-to-live in milliseconds.
    pub fn ttl(&self) -> Option<u64> {
        self.get(header::TTL)
    }

    /// MIME type of the body.
    pub fn content_type(&self) -> Option<&str> {
        self.get(header::CONTENT_TYPE)
    }

    /// Return address for replies.
    pub fn reply_to(&self) -> Option<&str> {
        self.get(header::REPLY_TO)
    }

    /// Links related messages across a conversation or task.
    pub fn correlation_id(&self) -> Option<&str> {
        self.get(header::CORRELATION_ID)
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
    pub fn build(mut self) -> Message {
        if !self.headers.contains_key(header::ID) {
            self.headers.insert(
                header::ID.to_string(),
                HeaderValue::String(Uuid::new_v4().to_string()),
            );
        }

        if !self.headers.contains_key(header::TIMESTAMP) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            self.headers
                .insert(header::TIMESTAMP.to_string(), HeaderValue::Integer(now));
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
        body: Vec<u8>,
        headers: HashMap<String, String>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

    /// Publish a request message and use the return address to await the reply.
    fn publish_request<'a>(
        &'a self,
        channel: &'a str,
        body: Vec<u8>,
        headers: HashMap<String, String>,
    ) -> ReplyFuture<'a>;
}

/// Return address for a request-reply exchange.
pub trait ReturnAddress: Send {
    /// The ephemeral channel name placed in the outgoing `reply-to` header.
    fn channel(&self) -> &str;

    /// Await the reply. Cleans up the ephemeral channel after receiving.
    fn take(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<Message>> + Send>>;
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
            .header(header::ID, "custom-id-1")
            .build();

        assert_eq!(msg.headers().id(), "custom-id-1");
    }

    #[test]
    fn builder_with_well_known_headers() {
        let msg = MessageBuilder::new(b"{}".to_vec())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CORRELATION_ID, "corr-1")
            .header(header::TTL, 5000_i64)
            .header(header::REPLY_TO, "reply-chan")
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
            header::CONTENT_TYPE.to_string(),
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
            .header(header::CONTENT_TYPE, "text/plain")
            .build();

        // At minimum: id, timestamp, content-type
        assert!(msg.headers().len() >= 3);

        let keys: Vec<&str> = msg.headers().iter().map(|(k, _)| k).collect();
        assert!(keys.contains(&header::ID));
        assert!(keys.contains(&header::TIMESTAMP));
        assert!(keys.contains(&header::CONTENT_TYPE));
    }
}
