//! Producing a component's bytes from the `uri` of its definition.

use anyhow::Result;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

/// Produces the bytes for a component definition's `uri`.
pub trait Resolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>>;
}

/// The resolvers available to a build, by uri scheme.
pub struct Resolvers {
    by_scheme: HashMap<&'static str, Box<dyn Resolver>>,
    default: FileResolver,
}

impl Resolvers {
    pub fn new(by_scheme: HashMap<&'static str, Box<dyn Resolver>>) -> Self {
        Self {
            by_scheme,
            default: FileResolver,
        }
    }

    /// Produce the bytes for `uri`. A uri whose scheme is not registered is
    /// read from the filesystem.
    pub async fn resolve(&self, uri: &str) -> Result<Vec<u8>> {
        let scheme = uri.split_once(':').map(|(scheme, _)| scheme).unwrap_or("");
        match self.by_scheme.get(scheme) {
            Some(resolver) => resolver.resolve(uri).await,
            None => self.default.resolve(uri).await,
        }
    }
}

/// Reads from the local filesystem: `file://` or a plain path.
pub struct FileResolver;

impl Resolver for FileResolver {
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let path = match uri.strip_prefix("file://") {
                Some(path) => PathBuf::from(path),
                None => PathBuf::from(uri),
            };
            Ok(std::fs::read(path)?)
        })
    }
}

/// Pulls from an OCI registry: `oci://`.
pub struct OciResolver;

impl Resolver for OciResolver {
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let oci_ref = uri
                .strip_prefix("oci://")
                .ok_or_else(|| anyhow::anyhow!("not an OCI uri: {uri}"))?;
            let client = wasm_pkg_client::oci::client::Client::new(Default::default());
            let image_ref = oci_ref.parse()?;
            let auth = oci_client::secrets::RegistryAuth::Anonymous;
            let media_types = vec!["application/wasm", "application/vnd.wasm.component"];

            let image_data = client.pull(&image_ref, &auth, media_types).await?;

            // The component bytes are the first layer.
            match image_data.layers.first() {
                Some(layer) => Ok(layer.data.to_vec()),
                None => Err(anyhow::anyhow!("No layers found in OCI image: {oci_ref}")),
            }
        })
    }
}
