//! Aspect-oriented programming for wasm components
//!
//! Given a target WIT world, creates an interceptor component that wraps
//! exported functions with before and after advice hooks. The interceptor,
//! target, and advice components can then be composed using standard wasm
//! component component model tooling, such as `wac`.
//!
//! # Entry points
//!
//! - [`create_from_wit`]: create from a WIT path (no target component required)
//! - [`create_from_component`]: create from a target component .wasm file

pub(crate) mod builder;
pub(crate) mod encoder;
pub(crate) mod extractor;
pub(crate) mod generator;
pub(crate) mod matcher;
pub(crate) mod types;

use std::path::Path;

use anyhow::Result;

use matcher::Pattern;
use types::*;

/// Create an interceptor component for a WIT world.
///
/// - `wit_path`: path to WIT file or directory
/// - `world`: world name whose exports define the interceptor contract
/// - `patterns`: match patterns for selective interception (empty = intercept all)
///
/// Returns the validated component bytes.
pub fn create_from_wit(wit_path: &Path, world: &str, patterns: &[&str]) -> Result<Vec<u8>> {
    let patterns = parse_patterns(patterns)?;
    let target = extractor::extract_from_wit(wit_path, world, &patterns)?;
    create_and_validate(target)
}

/// Create an interceptor component from an existing component binary.
///
/// Extracts the world from the component's embedded WIT, then creates an interceptor
/// for its exports.
///
/// - `component`: a valid wasm component binary
/// - `patterns`: match patterns for selective interception (empty = intercept all)
///
/// Returns the validated component bytes.
pub fn create_from_component(component: &[u8], patterns: &[&str]) -> Result<Vec<u8>> {
    let patterns = parse_patterns(patterns)?;
    let target = extractor::extract_from_wasm(component, &patterns)?;
    create_and_validate(target)
}

fn parse_patterns(patterns: &[&str]) -> Result<Vec<Pattern>> {
    patterns
        .iter()
        .map(|s| Pattern::parse(s))
        .collect::<Result<Vec<_>>>()
}

fn create_and_validate(target: TargetWorld) -> Result<Vec<u8>> {
    if target.exports.is_empty() {
        anyhow::bail!(
            "No exports found in world '{}'",
            target.resolve().worlds[target.world_id].name
        );
    }

    for we in &target.exports {
        match we {
            WorldExport::Interface(ie) => {
                let intercepted = ie
                    .functions
                    .iter()
                    .filter(|fe| matches!(fe, FunctionExport::Intercepted(_)))
                    .count();
                let bypassed = ie.functions.len() - intercepted;
                tracing::info!(
                    "{}: {} intercepted, {} bypassed",
                    ie.full_name,
                    intercepted,
                    bypassed
                );
            }
            WorldExport::Function(fe) => {
                let status = match fe {
                    FunctionExport::Intercepted(_) => "intercepted",
                    FunctionExport::Bypassed(_) => "bypassed",
                };
                tracing::info!("{}: {status}", fe.name());
            }
        }
    }

    let component_bytes = create(&target)?;

    wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(&component_bytes)
        .map_err(|e| anyhow::anyhow!("Validation failed: {e}"))?;

    tracing::info!("Validation passed");

    Ok(component_bytes)
}

// Create an interceptor component: generate modules, then build the component.
fn create(target: &TargetWorld) -> Result<Vec<u8>> {
    let (intercepted, core_bytes, shim_bytes, fixup_bytes) = generator::generate_modules(target)?;

    if intercepted.is_empty() {
        anyhow::bail!("No intercepted functions");
    }

    builder::build(target, &intercepted, core_bytes, shim_bytes, fixup_bytes)
}
