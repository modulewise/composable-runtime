//! A local store for component bytes pulled from an OCI registry.
//!
//! Content is addressed by the digest of the layer holding it, but a reference
//! names a tag or a manifest digest. An index of references to layer digests
//! removes the need for a network call when resolving a cached reference.
//!
//! ```text
//! <root>/
//!   refs/
//!     ghcr.io/foo/bar/v0.2.1      # holds "sha256:abc123..."
//!   blobs/
//!     sha256_abc123...            # the component bytes
//! ```

use anyhow::{Context, Result, bail};
use etcetera::BaseStrategy;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use wasm_pkg_client::ContentDigest;

/// Overrides the base dir for the cache.
const CACHE_DIR_VAR: &str = "COMPOSABLE_OCI_CACHE_DIR";

pub struct OciCache {
    root: PathBuf,
}

impl OciCache {
    /// `COMPOSABLE_OCI_CACHE_DIR` if set, otherwise a `composable` directory
    /// under `~/.cache`, or `%LOCALAPPDATA%` on Windows.
    pub fn new() -> Result<Self> {
        let root = match std::env::var_os(CACHE_DIR_VAR) {
            Some(dir) => PathBuf::from(dir),
            None => etcetera::choose_base_strategy()
                .context("locating the platform cache directory")?
                .cache_dir()
                .join("composable"),
        };
        Ok(Self { root })
    }

    /// The bytes cached for `reference`, if any. A missing or invalid blob is
    /// treated as a miss.
    pub fn get(&self, reference: &str) -> Option<Vec<u8>> {
        let entry = std::fs::read_to_string(self.ref_path(reference).ok()?).ok()?;
        let digest: ContentDigest = entry.trim().parse().ok()?;
        let bytes = std::fs::read(self.blob_path(&digest)).ok()?;

        if digest_of(&bytes) != digest {
            tracing::warn!("cached blob {digest} does not match its digest, ignoring it");
            return None;
        }
        Some(bytes)
    }

    /// Store `bytes` for `reference` under `digest`, which must be the digest
    /// the registry reported for the layer.
    pub fn put(&self, reference: &str, digest: &str, bytes: &[u8]) -> Result<()> {
        let digest: ContentDigest = digest
            .parse()
            .with_context(|| format!("registry reported an unusable digest for {reference}"))?;
        let actual = digest_of(bytes);
        if actual != digest {
            bail!("pulled bytes hash to {actual}, but the registry reported {digest}");
        }

        let blob_path = self.blob_path(&digest);
        // Concurrent writers produce the same bytes.
        if !blob_path.exists() {
            write_atomically(&blob_path, bytes)
                .with_context(|| format!("writing cached blob {digest}"))?;
        }

        let ref_path = self.ref_path(reference)?;
        write_atomically(&ref_path, digest.to_string().as_bytes())
            .with_context(|| format!("writing cache index entry for {reference}"))
    }

    fn blob_path(&self, digest: &ContentDigest) -> PathBuf {
        self.root
            .join("blobs")
            .join(digest.to_string().replace(':', "_"))
    }

    /// The index path for `reference`, one directory per segment.
    fn ref_path(&self, reference: &str) -> Result<PathBuf> {
        let mut path = self.root.join("refs");
        for segment in reference.split('/') {
            path.push(path_segment(segment)?);
        }
        Ok(path)
    }
}

/// Rejects anything that would escape the cache directory, and replaces `:`
/// since it is not legal on all platforms and appears in tagged references.
fn path_segment(segment: &str) -> Result<String> {
    if segment.is_empty() || segment == "." || segment == ".." {
        bail!("reference segment {segment:?} cannot be used as a path");
    }
    if segment.contains('\\') {
        bail!("reference segment {segment:?} cannot contain a backslash");
    }
    Ok(segment.replace(':', "_"))
}

fn digest_of(bytes: &[u8]) -> ContentDigest {
    Sha256::new_with_prefix(bytes).into()
}

/// Writes a temporary file and persists only if fully written.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent)?;

    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    std::io::Write::write_all(&mut file, bytes)?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> (OciCache, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let cache = OciCache {
            root: dir.path().to_path_buf(),
        };
        (cache, dir)
    }

    const REFERENCE: &str = "ghcr.io/foo/bar:v0.2.1";

    /// The digest as a registry would report it.
    fn digest(bytes: &[u8]) -> String {
        digest_of(bytes).to_string()
    }

    #[test]
    fn bytes_roundtrip() {
        let (cache, _dir) = cache();
        let bytes = b"component".to_vec();
        cache.put(REFERENCE, &digest(&bytes), &bytes).expect("put");
        assert_eq!(cache.get(REFERENCE), Some(bytes));
    }

    #[test]
    fn unknown_reference_misses() {
        let (cache, _dir) = cache();
        assert_eq!(cache.get(REFERENCE), None);
    }

    #[test]
    fn put_rejects_a_digest_that_does_not_match() {
        let (cache, _dir) = cache();
        let wrong = digest(b"something else");
        let err = cache
            .put(REFERENCE, &wrong, b"component")
            .expect_err("mismatched digest must not be stored");
        assert!(err.to_string().contains(&wrong), "{err}");
        assert_eq!(cache.get(REFERENCE), None);
    }

    #[test]
    fn put_rejects_a_malformed_digest() {
        let (cache, _dir) = cache();
        for malformed in ["sha256:0000", "deadbeef", "md5:abc"] {
            assert!(
                cache.put(REFERENCE, malformed, b"component").is_err(),
                "{malformed} should not be accepted as a digest"
            );
        }
    }

    #[test]
    fn missing_blob_reads_as_a_miss() {
        let (cache, _dir) = cache();
        let bytes = b"component".to_vec();
        cache.put(REFERENCE, &digest(&bytes), &bytes).expect("put");

        std::fs::remove_file(cache.blob_path(&digest_of(&bytes))).expect("remove blob");
        assert_eq!(
            cache.get(REFERENCE),
            None,
            "an index entry without its blob should re-pull, not fail"
        );
    }

    #[test]
    fn corrupted_blob_reads_as_a_miss() {
        let (cache, _dir) = cache();
        let bytes = b"component".to_vec();
        cache.put(REFERENCE, &digest(&bytes), &bytes).expect("put");

        std::fs::write(cache.blob_path(&digest_of(&bytes)), b"corrupted").expect("overwrite blob");
        assert_eq!(cache.get(REFERENCE), None);
    }

    #[test]
    fn references_sharing_a_layer_share_one_blob() {
        let (cache, _dir) = cache();
        let bytes = b"component".to_vec();
        cache.put(REFERENCE, &digest(&bytes), &bytes).expect("put");
        cache
            .put("ghcr.io/other/name:1.0.0", &digest(&bytes), &bytes)
            .expect("put");

        let blobs: Vec<_> = std::fs::read_dir(cache.root.join("blobs"))
            .expect("blobs dir")
            .collect();
        assert_eq!(
            blobs.len(),
            1,
            "identical content should be stored only once"
        );
        assert_eq!(cache.get("ghcr.io/other/name:1.0.0"), Some(bytes));
    }

    #[test]
    fn distinct_references_do_not_collide() {
        let (cache, _dir) = cache();
        let one = b"first".to_vec();
        let two = b"second".to_vec();
        cache.put("host/a_b/c:1", &digest(&one), &one).expect("put");
        cache.put("host/a/b_c:1", &digest(&two), &two).expect("put");

        assert_eq!(cache.get("host/a_b/c:1"), Some(one));
        assert_eq!(cache.get("host/a/b_c:1"), Some(two));
    }

    #[test]
    fn escaping_segments_are_rejected() {
        let (cache, _dir) = cache();
        for reference in ["../escape:1", "host/../../escape:1", "host//escape:1"] {
            assert!(
                cache.ref_path(reference).is_err(),
                "{reference} should not be usable as a path"
            );
        }
    }

    #[test]
    fn overwriting_a_reference_moves_it_to_the_new_digest() {
        let (cache, _dir) = cache();
        let old = b"old".to_vec();
        let new = b"new".to_vec();
        cache.put(REFERENCE, &digest(&old), &old).expect("put");
        cache.put(REFERENCE, &digest(&new), &new).expect("put");
        assert_eq!(cache.get(REFERENCE), Some(new));
    }
}
