//! HTTP Gateway for Composable Runtime.
//!
//! `HttpGatewayService` that implements the `Service` trait.
//! Handles `[gateway.*]` definitions with `type = "http"`.

mod config;
mod server;
mod service;

pub use service::HttpGatewayService;
