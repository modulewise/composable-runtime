//! HTTP Server extension for Composable Runtime.
//!
//! `HttpService` that implements the `Service` trait.
//! Handles `[server.*]` definitions with `type = "http"`.

mod config;
mod server;
mod service;

pub use service::HttpService;
