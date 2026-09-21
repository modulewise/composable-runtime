//! Producing a component's bytes from the `uri` of its definition.

use super::cache::OciCache;
use anyhow::{Context, Result, bail};
use oci_client::Reference;
use oci_client::client::{Client, ClientConfig};
use oci_client::secrets::RegistryAuth;
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use wasm_pkg_client::oci::OciRegistryConfig;
use wasm_pkg_client::{Config as RegistryConfigs, Registry};

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

const MEDIA_TYPES: [&str; 2] = ["application/wasm", "application/vnd.wasm.component"];

/// Opts in to mutable references for "latest" tags. Default is false.
const ALLOW_LATEST_VAR: &str = "COMPOSABLE_OCI_ALLOW_LATEST";

/// A registry configuration file. Falls back to `wasm-pkg`'s global config.
const REGISTRY_CONFIG_VAR: &str = "COMPOSABLE_OCI_REGISTRY_CONFIG";

/// Pulls from an OCI registry: `oci://`.
///
/// Clients are stored in a map per-registry since each has its own config.
/// A registry with no entry in the config, gets a default client (HTTPS and
/// anonymous). A tag other than `latest` is considered immutable, so a cache
/// hit for such a tag avoids any pull.
pub struct OciResolver {
    configs: RegistryConfigs,
    clients: RwLock<HashMap<Registry, Arc<Client>>>,
    cache: Option<OciCache>,
}

impl OciResolver {
    /// Reads registry configuration from `COMPOSABLE_OCI_REGISTRY_CONFIG`, or
    /// `wasm-pkg`'s global config. A registry serving plain HTTP can be
    /// declared with `protocol = "http"`.
    pub async fn new() -> Result<Self> {
        let configs = match std::env::var_os(REGISTRY_CONFIG_VAR) {
            Some(path) => RegistryConfigs::from_file(&path)
                .await
                .with_context(|| format!("reading {REGISTRY_CONFIG_VAR}"))?,
            None => RegistryConfigs::global_defaults()
                .await
                .context("reading wasm-pkg registry configuration")?,
        };
        let mut resolver = Self::with_configs(configs);
        match OciCache::new() {
            Ok(cache) => resolver.cache = Some(cache),
            Err(e) => tracing::warn!("no cache directory, so components are not cached: {e:#}"),
        }
        Ok(resolver)
    }

    fn with_configs(configs: RegistryConfigs) -> Self {
        Self {
            configs,
            clients: RwLock::new(HashMap::new()),
            cache: None,
        }
    }

    /// The client for `registry`, built from its configuration on first use.
    fn client(&self, registry: &Registry) -> Result<Arc<Client>> {
        if let Some(client) = self.clients.read().unwrap().get(registry) {
            return Ok(client.clone());
        }

        let config = match self.configs.registry_config(registry) {
            Some(registry_config) => {
                let OciRegistryConfig { client_config, .. } = registry_config
                    .try_into()
                    .with_context(|| format!("reading configuration for registry {registry}"))?;
                client_config
            }
            None => ClientConfig::default(),
        };
        if config.protocol == oci_client::client::ClientProtocol::Http {
            tracing::warn!("{registry} is configured for plain HTTP, so pulls are not encrypted");
        }
        let client = Arc::new(Client::new(config));

        let mut clients = self.clients.write().unwrap();
        // Another thread may have inserted while the read lock was released.
        Ok(clients.entry(registry.clone()).or_insert(client).clone())
    }
}

impl Resolver for OciResolver {
    fn resolve<'a>(
        &'a self,
        uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let oci_ref = uri
                .strip_prefix("oci://")
                .ok_or_else(|| anyhow::anyhow!("not an OCI uri: {uri}"))?;
            let image_ref: Reference = oci_ref.parse()?;

            let cache = match is_mutable(&image_ref) {
                // A mutable reference avoids the cache.
                true if allow_latest() => None,
                true => bail!(
                    "{uri} has no tag or is tagged `latest`, so it cannot be resolved to \
                     fixed bytes. Use an immutable tag, or set {ALLOW_LATEST_VAR}=1 to pull \
                     it on every run."
                ),
                false => self.cache.as_ref(),
            };

            if let Some(bytes) = cache.and_then(|cache| cache.get(oci_ref)) {
                tracing::debug!("{oci_ref} resolved from the cache");
                return Ok(bytes);
            }

            let registry: Registry = image_ref
                .resolve_registry()
                .parse()
                .with_context(|| format!("not a valid registry in: {uri}"))?;
            let client = self.client(&registry)?;

            tracing::debug!("pulling {oci_ref}");
            let image_data = client
                .pull(&image_ref, &RegistryAuth::Anonymous, MEDIA_TYPES.to_vec())
                .await?;

            // The component bytes are the first layer.
            let layer = image_data
                .layers
                .first()
                .ok_or_else(|| anyhow::anyhow!("No layers found in OCI image: {oci_ref}"))?;
            let bytes = layer.data.to_vec();

            if let Some(cache) = cache {
                // The layer's digest, as reported in the manifest.
                let digest = image_data
                    .manifest
                    .as_ref()
                    .and_then(|manifest| manifest.layers.first())
                    .map(|descriptor| descriptor.digest.as_str());
                match digest {
                    Some(digest) => {
                        if let Err(e) = cache.put(oci_ref, digest, &bytes) {
                            tracing::warn!("{oci_ref} was pulled but not cached: {e:#}");
                        }
                    }
                    None => {
                        tracing::warn!("{oci_ref} reported no layer digest, so it is not cached")
                    }
                }
            }
            Ok(bytes)
        })
    }
}

/// Whether the tag can be repointed to different content.
fn is_mutable(image_ref: &Reference) -> bool {
    image_ref.digest().is_none() && image_ref.tag().is_none_or(|tag| tag == "latest")
}

fn allow_latest() -> bool {
    std::env::var_os(ALLOW_LATEST_VAR).is_some_and(|value| value != "0" && value != "false")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::client::ClientProtocol;

    fn resolver(toml: &str) -> OciResolver {
        OciResolver::with_configs(RegistryConfigs::from_toml(toml).expect("valid config"))
    }

    /// The protocol for a registry's client, read from config since it's
    /// private on the client.
    fn protocol(resolver: &OciResolver, registry: &str) -> ClientProtocol {
        let registry: Registry = registry.parse().expect("valid registry");
        resolver.client(&registry).expect("client");
        resolver
            .configs
            .registry_config(&registry)
            .map(|config| {
                let OciRegistryConfig { client_config, .. } =
                    config.try_into().expect("valid oci config");
                client_config.protocol
            })
            .unwrap_or_default()
    }

    #[test]
    fn unconfigured_registry_defaults_to_https() {
        let resolver = resolver("");
        assert_eq!(protocol(&resolver, "ghcr.io"), ClientProtocol::Https);
    }

    #[test]
    fn configured_registry_can_use_plain_http() {
        let resolver = resolver(
            r#"
            [registry."localhost:5001"]
            type = "oci"
            [registry."localhost:5001".oci]
            protocol = "http"
            "#,
        );
        assert_eq!(
            protocol(&resolver, "localhost:5001"),
            ClientProtocol::Http,
            "a registry declared as http should not be pulled over https"
        );
        // Only the declared registry is insecure.
        assert_eq!(protocol(&resolver, "ghcr.io"), ClientProtocol::Https);
    }

    #[test]
    fn port_distinguishes_registries() {
        let resolver = resolver(
            r#"
            [registry."localhost:5001"]
            type = "oci"
            [registry."localhost:5001".oci]
            protocol = "http"
            "#,
        );
        assert_eq!(protocol(&resolver, "localhost:5002"), ClientProtocol::Https);
    }

    #[test]
    fn clients_are_reused_per_registry() {
        let resolver = resolver("");
        let registry: Registry = "ghcr.io".parse().expect("valid registry");
        let first = resolver.client(&registry).expect("client");
        let second = resolver.client(&registry).expect("client");
        assert!(
            Arc::ptr_eq(&first, &second),
            "a registry's client should be built once and reused"
        );
    }

    #[test]
    fn an_untagged_or_latest_reference_is_mutable() {
        for reference in [
            "ghcr.io/foo/bar",
            "ghcr.io/foo/bar:latest",
            "localhost:5001/foo/bar:latest",
        ] {
            let image_ref: Reference = reference.parse().expect("valid reference");
            assert!(
                is_mutable(&image_ref),
                "{reference} should be treated as latest"
            );
        }
    }

    #[test]
    fn a_nonlatest_tagged_or_digest_reference_is_immutable() {
        for reference in [
            "ghcr.io/foo/bar:v0.2.1",
            "ghcr.io/foo/bar:0.1.0",
            "localhost:5001/foo/bar:dev",
            "ghcr.io/foo/bar@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let image_ref: Reference = reference.parse().expect("valid reference");
            assert!(!is_mutable(&image_ref), "{reference} names fixed content");
        }
    }

    #[test]
    fn distinct_registries_get_distinct_clients() {
        let resolver = resolver("");
        let one = resolver
            .client(&"ghcr.io".parse().expect("valid registry"))
            .expect("client");
        let two = resolver
            .client(&"quay.io".parse().expect("valid registry"))
            .expect("client");
        assert!(!Arc::ptr_eq(&one, &two));
    }
}
