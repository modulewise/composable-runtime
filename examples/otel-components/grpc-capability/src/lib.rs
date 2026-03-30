//! Example of a generic gRPC endpoint host capability for composable-runtime.
//!
//! Implements `modulewise:grpc/endpoint`

use anyhow::Result;
use bytes::{Buf, BufMut};
use composable_runtime::{ComponentState, HostCapability};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tonic::transport::Channel;
use wasmtime::component::{HasSelf, Linker};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "grpc-sender",
        imports: { default: async | trappable },
    });
}

use bindings::modulewise::grpc::endpoint;

struct EndpointState {
    channel: Channel,
    paths: HashMap<String, String>,
}

impl EndpointState {
    fn new(url: &str, paths: HashMap<String, String>) -> Result<Self> {
        let channel = Channel::from_shared(url.to_string())?.connect_lazy();

        Ok(Self { channel, paths })
    }
}

/// Host capability that provides the `modulewise:grpc/endpoint` interface.
#[derive(Deserialize)]
pub struct GrpcCapability {
    #[serde(default = "default_url")]
    pub url: String,

    #[serde(default)]
    pub paths: HashMap<String, String>,

    #[serde(skip)]
    state: OnceLock<Arc<EndpointState>>,
}

fn default_url() -> String {
    "http://localhost:4317".to_string()
}

impl Default for GrpcCapability {
    fn default() -> Self {
        Self {
            url: default_url(),
            paths: HashMap::new(),
            state: OnceLock::new(),
        }
    }
}

struct ComponentEndpointState {
    inner: Arc<EndpointState>,
}

impl HostCapability for GrpcCapability {
    fn interfaces(&self) -> Vec<String> {
        vec!["modulewise:grpc/endpoint@0.1.0".to_string()]
    }

    fn link(&self, linker: &mut Linker<ComponentState>) -> wasmtime::Result<()> {
        endpoint::add_to_linker::<_, HasSelf<_>>(linker, |state| state)
    }

    composable_runtime::create_state!(this, ComponentEndpointState, {
        let inner = this
            .state
            .get_or_init(|| {
                Arc::new(
                    EndpointState::new(&this.url, this.paths.clone())
                        .expect("EndpointState should be created"),
                )
            })
            .clone();

        ComponentEndpointState { inner }
    });
}

impl endpoint::Host for ComponentState {
    async fn send(&mut self, path: String, data: Vec<u8>) -> wasmtime::Result<Result<(), String>> {
        let state = self
            .get_extension::<ComponentEndpointState>()
            .expect("ComponentEndpointState should be initialized");

        let channel = state.inner.channel.clone();
        let grpc_path = match state.inner.paths.get(&path) {
            Some(p) => p.clone(),
            None => {
                return Ok(Err(format!("Unknown path: {}", path)));
            }
        };

        match send_grpc_request(channel, &grpc_path, data).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Failed to send gRPC data to path '{}': {}", path, e);
                Ok(Err(e.to_string()))
            }
        }
    }
}

async fn send_grpc_request(channel: Channel, path: &str, data: Vec<u8>) -> Result<()> {
    use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
    use tonic::codegen::http::uri::PathAndQuery;

    #[derive(Debug, Clone, Default)]
    struct RawCodec;

    impl Codec for RawCodec {
        type Encode = Vec<u8>;
        type Decode = Vec<u8>;
        type Encoder = RawEncoder;
        type Decoder = RawDecoder;
        fn encoder(&mut self) -> RawEncoder {
            RawEncoder
        }
        fn decoder(&mut self) -> RawDecoder {
            RawDecoder
        }
    }

    #[derive(Debug, Clone, Default)]
    struct RawEncoder;

    impl Encoder for RawEncoder {
        type Item = Vec<u8>;
        type Error = tonic::Status;
        fn encode(&mut self, item: Vec<u8>, dst: &mut EncodeBuf<'_>) -> Result<(), tonic::Status> {
            dst.reserve(item.len());
            dst.put_slice(&item);
            Ok(())
        }
    }

    #[derive(Debug, Clone, Default)]
    struct RawDecoder;

    impl Decoder for RawDecoder {
        type Item = Vec<u8>;
        type Error = tonic::Status;
        fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Vec<u8>>, tonic::Status> {
            let data = src.chunk().to_vec();
            src.advance(data.len());
            Ok(Some(data))
        }
    }

    let path: PathAndQuery = path.parse()?;
    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await?;

    let request = tonic::Request::new(data);
    let _response: tonic::Response<Vec<u8>> = client.unary(request, path, RawCodec).await?;

    Ok(())
}
