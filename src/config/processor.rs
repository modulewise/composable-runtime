use anyhow::Result;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use super::handlers::{CapabilityConfigHandler, ComponentConfigHandler};
use super::types::{ConfigHandler, Definition, DefinitionLoader, GenericDefinition, PropertyMap};
use crate::types::{CapabilityDefinition, ComponentDefinition};

/// Maximum number of handler-produced definitions that can be chained.
const MAX_DEPTH: usize = 8;

pub struct ConfigProcessor {
    loaders: Vec<Box<dyn DefinitionLoader>>,
    handlers: Vec<Box<dyn ConfigHandler>>,
}

impl ConfigProcessor {
    pub fn new() -> Self {
        Self {
            loaders: Vec::new(),
            handlers: Vec::new(),
        }
    }

    pub fn add_loader(&mut self, loader: Box<dyn DefinitionLoader>) {
        self.loaders.push(loader);
    }

    pub fn add_handler(&mut self, handler: Box<dyn ConfigHandler>) {
        self.handlers.push(handler);
    }

    /// Route paths to loaders via claim, then run the full config pipeline.
    pub fn process(
        mut self,
        paths: &[PathBuf],
    ) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
        // Route paths to loaders
        for path in paths {
            let mut claimed_by = Vec::new();
            for (idx, loader) in self.loaders.iter_mut().enumerate() {
                if loader.claim(path) {
                    claimed_by.push(idx);
                }
            }
            match claimed_by.len() {
                0 => {
                    return Err(anyhow::anyhow!(
                        "No loader can handle path: {}",
                        path.display()
                    ));
                }
                1 => {}
                _ => {
                    return Err(anyhow::anyhow!(
                        "Multiple loaders claimed path: {}",
                        path.display()
                    ));
                }
            }
        }

        // Collect definitions from all loaders
        let mut definitions = Vec::new();
        for loader in &self.loaders {
            definitions.extend(loader.load()?);
        }
        for definition in &definitions {
            validate_loaded_name(definition)?;
        }

        // Build unified handler collection: core handlers + registered handlers
        let mut all_handlers: Vec<Box<dyn ConfigHandler>> = Vec::new();
        all_handlers.push(Box::new(ComponentConfigHandler));
        all_handlers.push(Box::new(CapabilityConfigHandler));
        all_handlers.extend(self.handlers);

        let (mut component_definitions, mut capability_definitions) =
            dispatch(definitions, &mut all_handlers)?;

        // Resolve placeholders
        resolve_placeholders_in_components(&mut component_definitions)?;
        resolve_placeholders_in_capabilities(&mut capability_definitions)?;

        // Cross-definition validation
        validate_scopes(&component_definitions, &capability_definitions)?;
        validate_names(&component_definitions, &capability_definitions)?;
        validate_imports(&component_definitions, &capability_definitions)?;
        validate_wit_references(&component_definitions, &capability_definitions)?;

        Ok((component_definitions, capability_definitions))
    }
}

use super::types::CategoryClaim;

// A category claim registered by a handler, with its handler index.
struct RegisteredClaim {
    handler_idx: usize,
    claim: CategoryClaim,
}

// A generic definition to dispatch, with the chain of definitions that
// produced it, most recent first.
struct Queued {
    definition: GenericDefinition,
    origin: Vec<String>,
}

// Dispatch every definition to the handler that claims its category,
// including definitions produced by handlers themselves, until only
// components and capabilities remain.
fn dispatch(
    definitions: Vec<GenericDefinition>,
    handlers: &mut [Box<dyn ConfigHandler + '_>],
) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
    // Build category => list of claims (handler index + optional selector).
    // Validate: if any claim on a category has no selector, it must be the only claim.
    let mut category_claims: HashMap<String, Vec<RegisteredClaim>> = HashMap::new();
    for (idx, handler) in handlers.iter().enumerate() {
        for claim in handler.claimed_categories() {
            category_claims
                .entry(claim.category.to_string())
                .or_default()
                .push(RegisteredClaim {
                    handler_idx: idx,
                    claim,
                });
        }
    }
    for (category, claims) in &category_claims {
        if claims.len() > 1 {
            let has_unselected = claims.iter().any(|c| c.claim.selector.is_none());
            if has_unselected {
                return Err(anyhow::anyhow!(
                    "Category '{category}' claimed exclusively (without a selector) and by other handlers"
                ));
            }
        }
    }

    // The properties each handler claims on a category it owns, keyed by
    // handler index and category name. Handlers sharing a category through
    // selectors never handle the same definition, so these may overlap.
    let mut owned_properties: HashMap<(usize, String), HashSet<String>> = HashMap::new();

    // The handler that claims each property on a category it does not own,
    // keyed by category name and property name. The property is routed to that
    // handler, so each has exactly one handler id.
    let mut contribution_handlers: HashMap<(String, String), usize> = HashMap::new();
    for (idx, handler) in handlers.iter().enumerate() {
        let claimed: HashSet<&str> = handler
            .claimed_categories()
            .into_iter()
            .map(|claim| claim.category)
            .collect();
        for (category, properties) in handler.claimed_properties() {
            if claimed.contains(category) {
                owned_properties
                    .entry((idx, category.to_string()))
                    .or_default()
                    .extend(properties.iter().map(|prop| prop.to_string()));
                continue;
            }
            for prop in properties {
                let key = (category.to_string(), prop.to_string());
                if let Some(&existing_idx) = contribution_handlers.get(&key)
                    && existing_idx != idx
                {
                    return Err(anyhow::anyhow!(
                        "Property '{prop}' on category '{category}' claimed by multiple handlers"
                    ));
                }
                contribution_handlers.insert(key, idx);
            }
        }
    }
    // A contributed property must not also be owned.
    for (category, prop) in contribution_handlers.keys() {
        let is_owned = owned_properties
            .iter()
            .any(|((_, owned), properties)| owned == category && properties.contains(prop));
        if is_owned {
            return Err(anyhow::anyhow!(
                "Property '{prop}' on category '{category}' is claimed by both the category's owner and another handler"
            ));
        }
    }

    let mut components = Vec::new();
    let mut capabilities = Vec::new();
    let mut queue: VecDeque<Queued> = definitions
        .into_iter()
        .map(|definition| Queued {
            definition,
            origin: Vec::new(),
        })
        .collect();

    while let Some(Queued { definition, origin }) = queue.pop_front() {
        let generated = generated_by(&origin);
        let claims = category_claims.get(&definition.category).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown category '{}'{generated}. Known categories: {:?}",
                definition.category,
                category_claims.keys().collect::<Vec<_>>()
            )
        })?;

        let owner_idx =
            resolve_owner(claims, &definition).map_err(|e| anyhow::anyhow!("{e}{generated}"))?;

        let GenericDefinition {
            category,
            name,
            properties,
        } = definition;
        let label = format!("[{category}.{name}]");
        let (core_properties, claimed_by_handler) =
            split_properties(properties, &category, &contribution_handlers);

        // Reject any remaining provided property this definition's owner does
        // not claim, unless it accepts unclaimed properties as pass-through
        // configuration (e.g. capability handlers forwarding type-specific keys).
        if !handlers[owner_idx].accepts_unclaimed_properties(&category) {
            let owned = owned_properties.get(&(owner_idx, category.clone()));
            for key in core_properties.keys() {
                if !owned.is_some_and(|properties| properties.contains(key)) {
                    return Err(anyhow::anyhow!(
                        "Category '{category}' definition '{name}' has unknown property '{key}'{generated}"
                    ));
                }
            }
        }

        let produced_definitions = handlers[owner_idx]
            .handle_definition(GenericDefinition {
                category: category.clone(),
                name: name.clone(),
                properties: core_properties,
            })
            .map_err(|e| anyhow::anyhow!("{e}{generated}"))?;

        for (handler_idx, properties) in claimed_by_handler {
            handlers[handler_idx].handle_properties(&category, &name, properties)?;
        }

        for produced_definition in produced_definitions {
            match produced_definition {
                Definition::Component(def) => components.push(def),
                Definition::Capability(def) => capabilities.push(def),
                Definition::Generic(definition) => {
                    let mut chain = vec![label.clone()];
                    chain.extend(origin.iter().cloned());
                    if chain.len() > MAX_DEPTH {
                        return Err(anyhow::anyhow!(
                            "[{}.{}] is more than {MAX_DEPTH} definitions deep{}, which is the max depth",
                            definition.category,
                            definition.name,
                            generated_by(&chain)
                        ));
                    }
                    queue.push_back(Queued {
                        definition,
                        origin: chain,
                    });
                }
            }
        }
    }

    Ok((components, capabilities))
}

// Formatted origin string for a definition produced by handlers. The closest
// is first. Empty string for a definition provided directly by a loader.
fn generated_by(origin: &[String]) -> String {
    match origin.split_first() {
        None => String::new(),
        Some((closest, [])) => format!(" (generated by {closest})"),
        Some((closest, rest)) => {
            format!(" (generated by {closest}, from {})", rest.join(", from "))
        }
    }
}

// Find the single handler that owns a definition.
// Returns an error if not exactly one match.
fn resolve_owner(claims: &[RegisteredClaim], def: &GenericDefinition) -> Result<usize> {
    // Single claim with no selector => unconditional category owner
    if claims.len() == 1 && claims[0].claim.selector.is_none() {
        return Ok(claims[0].handler_idx);
    }

    // Flatten properties to string-to-string for selector matching
    let flat = flatten_for_selector(&def.properties);

    let mut matched = Vec::new();
    for rc in claims {
        let matches = rc.claim.selector.as_ref().is_none_or(|s| s.matches(&flat));
        if matches {
            matched.push(rc.handler_idx);
        }
    }

    match matched.len() {
        0 => Err(anyhow::anyhow!(
            "No handler matched definition '{}' in category '{}'",
            def.name,
            def.category
        )),
        1 => Ok(matched[0]),
        _ => Err(anyhow::anyhow!(
            "Multiple handlers matched definition '{}' in category '{}'",
            def.name,
            def.category
        )),
    }
}

// Flatten a PropertyMap for selector matching. Scalar values become
// Some(string), arrays and null become None (key present but no scalar value).
// Nested objects use dot-delimited keys.
fn flatten_for_selector(properties: &PropertyMap) -> HashMap<String, Option<String>> {
    let mut result = HashMap::new();
    flatten_recursive(properties, "", &mut result);
    result
}

fn flatten_recursive(
    map: &PropertyMap,
    prefix: &str,
    result: &mut HashMap<String, Option<String>>,
) {
    for (key, value) in map {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            serde_json::Value::String(s) => {
                result.insert(full_key, Some(s.clone()));
            }
            serde_json::Value::Bool(b) => {
                result.insert(full_key, Some(b.to_string()));
            }
            serde_json::Value::Number(n) => {
                result.insert(full_key, Some(n.to_string()));
            }
            serde_json::Value::Object(obj) => {
                let nested: PropertyMap = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                flatten_recursive(&nested, &full_key, result);
            }
            serde_json::Value::Array(_) | serde_json::Value::Null => {
                result.insert(full_key, None);
            }
        }
    }
}

// Split off the contributed properties, by the handler that claims each.
fn split_properties(
    mut properties: PropertyMap,
    category: &str,
    contribution_handlers: &HashMap<(String, String), usize>,
) -> (PropertyMap, HashMap<usize, PropertyMap>) {
    let mut claimed: HashMap<usize, PropertyMap> = HashMap::new();

    let keys: Vec<String> = properties.keys().cloned().collect();
    for key in keys {
        let lookup = (category.to_string(), key.clone());
        if let Some(&handler_idx) = contribution_handlers.get(&lookup)
            && let Some(value) = properties.remove(&key)
        {
            claimed.entry(handler_idx).or_default().insert(key, value);
        }
    }

    (properties, claimed)
}

// --- Placeholder resolvers ---

fn resolve_placeholders_in_components(definitions: &mut [ComponentDefinition]) -> Result<()> {
    for def in definitions {
        resolve_placeholders_in_map(&mut def.config)?;
    }
    Ok(())
}

fn resolve_placeholders_in_capabilities(definitions: &mut [CapabilityDefinition]) -> Result<()> {
    for def in definitions {
        resolve_placeholders_in_map(&mut def.properties)?;
    }
    Ok(())
}

fn resolve_placeholders_in_map(map: &mut HashMap<String, serde_json::Value>) -> Result<()> {
    for value in map.values_mut() {
        resolve_placeholders_in_value(value)?;
    }
    Ok(())
}

fn resolve_placeholders_in_value(value: &mut serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::String(s) => {
            if let Some(resolved) = resolve_placeholder(s)? {
                *s = resolved;
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                resolve_placeholders_in_value(item)?;
            }
        }
        serde_json::Value::Object(map) => {
            for v in map.values_mut() {
                resolve_placeholders_in_value(v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The component named by a `${wit(<component>)}` expression, if present.
pub(crate) fn wit_reference(value: &str) -> Option<&str> {
    value.strip_prefix("${wit(")?.strip_suffix(")}")
}

/// Every component named in a `${wit(...)}` expression within `config`.
pub(crate) fn wit_references(config: &HashMap<String, serde_json::Value>) -> Vec<&str> {
    let mut names = Vec::new();
    for value in config.values() {
        collect_wit_references(value, &mut names);
    }
    names
}

fn collect_wit_references<'a>(value: &'a serde_json::Value, names: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(s) => names.extend(wit_reference(s)),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_wit_references(item, names);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                collect_wit_references(item, names);
            }
        }
        _ => {}
    }
}

fn resolve_placeholder(s: &str) -> Result<Option<String>> {
    if !s.starts_with("${") || !s.ends_with('}') {
        return Ok(None);
    }

    // Evaluated during the build, once the named component exists.
    if wit_reference(s).is_some() {
        return Ok(None);
    }

    let inner = &s[2..s.len() - 1];
    if let Some(env_expr) = inner.strip_prefix("process:env:") {
        let (env_key, default) = match env_expr.split_once('|') {
            Some((key, default)) => (key, Some(default)),
            None => (env_expr, None),
        };
        match (std::env::var(env_key), default) {
            (Ok(value), _) => Ok(Some(value)),
            (Err(_), Some(default)) => Ok(Some(default.to_string())),
            (Err(_), None) => Err(anyhow::anyhow!(
                "Environment variable '{env_key}' not set (referenced in config placeholder '{s}')"
            )),
        }
    } else {
        Err(anyhow::anyhow!("Unknown placeholder pattern: '{s}'"))
    }
}

// --- Cross-definition validation ---

// TODO: replace with label selector validation
fn validate_scopes(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    for def in capabilities {
        match def.scope.as_str() {
            "any" => {}
            "package" | "namespace" => {
                return Err(anyhow::anyhow!(
                    "Capability '{}' cannot use scope='{}' - only components support package/namespace scoping",
                    def.name,
                    def.scope
                ));
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Invalid scope: '{}'. Must be: any",
                    def.scope
                ));
            }
        }
    }
    for def in components {
        match def.scope.as_str() {
            "any" | "package" | "namespace" => {}
            _ => {
                return Err(anyhow::anyhow!(
                    "Invalid scope: '{}'. Must be one of: any, package, namespace",
                    def.scope
                ));
            }
        }
    }
    Ok(())
}

fn validate_names(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    let mut all_names = HashSet::new();
    for def in capabilities {
        validate_name_chars(&def.name)?;
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    for def in components {
        validate_name_chars(&def.name)?;
        if !all_names.insert(&def.name) {
            return Err(anyhow::anyhow!("Duplicate definition name: '{}'", def.name));
        }
    }
    Ok(())
}

// A name with a leading `_` is internal (hidden from listing and direct
// invocation) and can only be defined by a config handler. Other config may
// still refer to such a definition by name, e.g. to import it.
fn validate_loaded_name(definition: &GenericDefinition) -> Result<()> {
    if definition.name.starts_with('_') {
        return Err(anyhow::anyhow!(
            "Definition name '{}' in category '{}' is invalid: names starting with '_' are reserved for definitions that config handlers generate",
            definition.name,
            definition.category
        ));
    }
    Ok(())
}

fn validate_name_chars(name: &str) -> Result<()> {
    if name.contains('$') {
        return Err(anyhow::anyhow!(
            "Definition name '{name}' is invalid: names cannot contain '$' (reserved for internal use)"
        ));
    }
    Ok(())
}

fn validate_wit_references(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    // Capabilities are built before components.
    for def in capabilities {
        if !wit_references(&def.properties).is_empty() {
            return Err(anyhow::anyhow!(
                "Capability '{}' has ${{wit(...)}}, which is only valid in component config",
                def.name
            ));
        }
    }

    let component_names: HashSet<&str> = components.iter().map(|d| d.name.as_str()).collect();
    for def in components {
        for name in wit_references(&def.config) {
            validate_name_chars(name)
                .map_err(|e| anyhow::anyhow!("Invalid ${{wit(...)}} in '{}': {e}", def.name))?;
            if !component_names.contains(name) {
                return Err(anyhow::anyhow!(
                    "Component '{}' uses ${{wit({name})}}, but no component '{name}' is defined",
                    def.name
                ));
            }
        }
    }
    Ok(())
}

fn validate_imports(
    components: &[ComponentDefinition],
    capabilities: &[CapabilityDefinition],
) -> Result<()> {
    let all_names: HashSet<&str> = components
        .iter()
        .map(|d| d.name.as_str())
        .chain(capabilities.iter().map(|d| d.name.as_str()))
        .collect();

    for def in components {
        for import_name in &def.imports {
            validate_name_chars(import_name)
                .map_err(|e| anyhow::anyhow!("Invalid import for '{}': {e}", def.name))?;
            if !all_names.contains(import_name.as_str()) {
                return Err(anyhow::anyhow!(
                    "Component '{}' imports undefined definition '{}'",
                    def.name,
                    import_name
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{CategoryClaim, Condition, Operator, Selector};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    /// Provides fixed definitions, as a file loader would.
    struct Fixed(Vec<GenericDefinition>);

    impl DefinitionLoader for Fixed {
        fn load(&self) -> Result<Vec<GenericDefinition>> {
            Ok(self.0.clone())
        }
    }

    fn generic(category: &str, name: &str, properties: serde_json::Value) -> GenericDefinition {
        let serde_json::Value::Object(properties) = properties else {
            panic!("properties must be an object");
        };
        GenericDefinition {
            category: category.to_string(),
            name: name.to_string(),
            properties: properties.into_iter().collect(),
        }
    }

    /// Expands each `[widget.NAME]` into `[component.NAME]`, importing
    /// `_shared`, which it emits once. When `stray` is `true`, it also emits a
    /// definition in a category no handler claims.
    #[derive(Default)]
    struct Widget {
        shared_emitted: bool,
        stray: bool,
    }

    impl ConfigHandler for Widget {
        fn claimed_categories(&self) -> Vec<CategoryClaim> {
            vec![CategoryClaim::all("widget")]
        }

        fn handle_definition(&mut self, definition: GenericDefinition) -> Result<Vec<Definition>> {
            let mut produced = vec![Definition::Generic(generic(
                "component",
                &definition.name,
                json!({ "uri": "widget.wasm", "imports": ["_shared"] }),
            ))];
            if !self.shared_emitted {
                self.shared_emitted = true;
                produced.push(Definition::Generic(generic(
                    "component",
                    "_shared",
                    json!({ "uri": "shared.wasm" }),
                )));
            }
            if self.stray {
                produced.push(Definition::Generic(generic(
                    "unclaimed",
                    "stray",
                    json!({}),
                )));
            }
            Ok(produced)
        }
    }

    /// Handles each `[loop.NAME]` by returning the same definition each time.
    struct Loop;

    impl ConfigHandler for Loop {
        fn claimed_categories(&self) -> Vec<CategoryClaim> {
            vec![CategoryClaim::all("loop")]
        }

        fn handle_definition(&mut self, definition: GenericDefinition) -> Result<Vec<Definition>> {
            Ok(vec![Definition::Generic(definition)])
        }
    }

    fn process(
        definitions: Vec<GenericDefinition>,
        handler: Box<dyn ConfigHandler>,
    ) -> Result<(Vec<ComponentDefinition>, Vec<CapabilityDefinition>)> {
        let mut processor = ConfigProcessor::new();
        processor.add_loader(Box::new(Fixed(definitions)));
        processor.add_handler(handler);
        processor.process(&[])
    }

    #[test]
    fn generic_definitions_are_dispatched_until_only_components_remain() {
        let (components, _) = process(
            vec![
                generic("widget", "first", json!({})),
                generic("widget", "second", json!({})),
                // Config may import an internal definition by name.
                generic(
                    "component",
                    "gadget",
                    json!({ "uri": "gadget.wasm", "imports": ["_shared"] }),
                ),
            ],
            Box::new(Widget::default()),
        )
        .unwrap();
        let mut names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["_shared", "first", "gadget", "second"]);
    }

    #[test]
    fn an_unclaimed_generated_definition_names_its_origin() {
        let error = process(
            vec![generic("widget", "first", json!({}))],
            Box::new(Widget {
                stray: true,
                ..Default::default()
            }),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Unknown category 'unclaimed'"), "{error}");
        assert!(error.contains("(generated by [widget.first])"), "{error}");
    }

    #[test]
    fn a_chain_deeper_than_the_limit_fails() {
        let error = process(vec![generic("loop", "again", json!({}))], Box::new(Loop))
            .unwrap_err()
            .to_string();
        assert!(error.contains("which is the max depth"), "{error}");
    }

    #[test]
    fn config_may_not_define_an_internal_name() {
        let error = process(
            vec![generic(
                "component",
                "_reserved",
                json!({ "uri": "reserved.wasm" }),
            )],
            Box::new(Loop),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("reserved for definitions that config handlers generate"),
            "{error}"
        );
    }

    /// What a test handler received: each definition's name and properties.
    type Received = Arc<Mutex<Vec<(String, Vec<String>)>>>;

    fn record(received: &Received, name: &str, properties: &PropertyMap) {
        let mut keys: Vec<String> = properties.keys().cloned().collect();
        keys.sort();
        received.lock().unwrap().push((name.to_string(), keys));
    }

    /// Claims `[server.*]` definitions whose `type` is `kind`, owning the
    /// `type` and `port` properties.
    struct Server {
        kind: &'static str,
        received: Received,
    }

    impl Server {
        fn new(kind: &'static str) -> (Self, Received) {
            let received = Received::default();
            let server = Server {
                kind,
                received: Arc::clone(&received),
            };
            (server, received)
        }
    }

    impl ConfigHandler for Server {
        fn claimed_categories(&self) -> Vec<CategoryClaim> {
            vec![CategoryClaim::with_selector(
                "server",
                Selector {
                    conditions: vec![Condition {
                        key: "type".to_string(),
                        operator: Operator::Equals(self.kind.to_string()),
                    }],
                },
            )]
        }

        fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
            HashMap::from([("server", ["type", "port"].as_slice())])
        }

        fn handle_definition(&mut self, definition: GenericDefinition) -> Result<Vec<Definition>> {
            record(&self.received, &definition.name, &definition.properties);
            Ok(Vec::new())
        }
    }

    /// Expands each `[indirect.NAME]` into a `[server.NAME]` of type `b`.
    struct Indirect;

    impl ConfigHandler for Indirect {
        fn claimed_categories(&self) -> Vec<CategoryClaim> {
            vec![CategoryClaim::all("indirect")]
        }

        fn handle_definition(&mut self, definition: GenericDefinition) -> Result<Vec<Definition>> {
            Ok(vec![Definition::Generic(generic(
                "server",
                &definition.name,
                json!({ "type": "b", "port": 2 }),
            ))])
        }
    }

    /// Contributes the `property` property to `[server.*]` definitions,
    /// without owning the category.
    struct Contributor {
        property: &'static [&'static str],
        received: Received,
    }

    impl ConfigHandler for Contributor {
        fn claimed_categories(&self) -> Vec<CategoryClaim> {
            Vec::new()
        }

        fn claimed_properties(&self) -> HashMap<&str, &[&str]> {
            HashMap::from([("server", self.property)])
        }

        fn handle_definition(&mut self, _: GenericDefinition) -> Result<Vec<Definition>> {
            unreachable!("claims no category")
        }

        fn handle_properties(
            &mut self,
            _category: &str,
            name: &str,
            properties: PropertyMap,
        ) -> Result<()> {
            record(&self.received, name, &properties);
            Ok(())
        }
    }

    #[test]
    fn a_category_claimed_by_selectors_goes_to_the_matching_handler() {
        let ((a, a_received), (b, b_received)) = (Server::new("a"), Server::new("b"));
        let mut processor = ConfigProcessor::new();
        processor.add_loader(Box::new(Fixed(vec![
            generic("server", "first", json!({ "type": "a", "port": 1 })),
            generic("server", "second", json!({ "type": "b", "port": 2 })),
            generic("indirect", "third", json!({})),
        ])));
        // Both own `type` and `port` on the category they share.
        processor.add_handler(Box::new(a));
        processor.add_handler(Box::new(b));
        processor.add_handler(Box::new(Indirect));
        processor.process(&[]).unwrap();

        let names = |received: &Received| -> Vec<String> {
            received
                .lock()
                .unwrap()
                .iter()
                .map(|(name, _)| name.clone())
                .collect()
        };
        assert_eq!(names(&a_received), ["first"]);
        // Including a definition another handler generated.
        assert_eq!(names(&b_received), ["second", "third"]);
    }

    #[test]
    fn a_contributed_property_goes_to_its_contributor_exclusively() {
        let (server, server_received) = Server::new("a");
        let contributed = Received::default();
        let mut processor = ConfigProcessor::new();
        processor.add_loader(Box::new(Fixed(vec![generic(
            "server",
            "first",
            json!({ "type": "a", "port": 1, "note": "hello" }),
        )])));
        processor.add_handler(Box::new(server));
        processor.add_handler(Box::new(Contributor {
            property: &["note"],
            received: Arc::clone(&contributed),
        }));
        processor.process(&[]).unwrap();

        let keys = |received: &Received| received.lock().unwrap()[0].1.clone();
        assert_eq!(keys(&server_received), ["port", "type"]);
        assert_eq!(keys(&contributed), ["note"]);
    }

    #[test]
    fn a_contributed_property_may_not_also_be_owned() {
        let (server, _) = Server::new("a");
        let mut processor = ConfigProcessor::new();
        processor.add_loader(Box::new(Fixed(Vec::new())));
        processor.add_handler(Box::new(server));
        processor.add_handler(Box::new(Contributor {
            property: &["port"],
            received: Received::default(),
        }));
        let error = processor.process(&[]).unwrap_err().to_string();
        assert!(
            error.contains("claimed by both the category's owner and another handler"),
            "{error}"
        );
    }
}
