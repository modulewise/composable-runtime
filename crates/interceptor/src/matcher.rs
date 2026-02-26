use anyhow::{Result, bail};

/// A parsed match pattern for selecting WIT functions to intercept.
///
/// Format: `namespace:package/interface@version#function`
/// Each segment can be `*` (wildcard) or omitted (matches all).
///
/// A delimiter-free pattern like `foo` targets the function name.
#[derive(Debug, Clone)]
pub struct Pattern {
    pub namespace: Segment,
    pub package: Segment,
    pub interface: Segment,
    pub version: Segment,
    pub function: Segment,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    Literal(String),
    Wildcard,
    Any,
}

impl Segment {
    fn matches(&self, value: &str) -> bool {
        match self {
            Segment::Literal(s) => {
                if s.contains('*') {
                    glob_match(s, value)
                } else {
                    s == value
                }
            }
            Segment::Wildcard | Segment::Any => true,
        }
    }

    fn is_any(&self) -> bool {
        matches!(self, Segment::Any)
    }
}

// Simple glob matching: `*` matches any sequence of characters.
// Currently does not support `?` or character classes.
fn glob_match(pattern: &str, value: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let mut pos = 0;

    // First part must match at the start
    if !parts[0].is_empty() {
        if !value.starts_with(parts[0]) {
            return false;
        }
        pos = parts[0].len();
    }

    // Middle parts must appear in order
    for part in &parts[1..parts.len() - 1] {
        if part.is_empty() {
            continue;
        }
        match value[pos..].find(part) {
            Some(idx) => pos += idx + part.len(),
            None => return false,
        }
    }

    // Last part must match at the end
    let last = parts[parts.len() - 1];
    if !last.is_empty() {
        value[pos..].ends_with(last)
    } else {
        true
    }
}

fn parse_segment(s: &str) -> Segment {
    if s == "*" {
        Segment::Wildcard
    } else {
        Segment::Literal(s.to_string())
    }
}

impl Pattern {
    /// Parse a match pattern string.
    ///
    /// Examples:
    /// - `modulewise:example/handler@0.2.0#handle` — specific function
    /// - `modulewise:example/handler` — all functions on an interface, any version
    /// - `modulewise:example/*` — all interfaces in a package
    /// - `modulewise:*` — everything in a namespace
    /// - `*` — match all
    /// - `foo` — function named "foo" (in any interface or as a direct function)
    /// - `say-*` — glob match on function name
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            bail!("Empty match pattern");
        }

        let (rest, function) = if let Some((r, f)) = s.rsplit_once('#') {
            (r, parse_segment(f))
        } else {
            (s, Segment::Any)
        };

        let (rest, version) = if let Some((r, v)) = rest.rsplit_once('@') {
            (r, parse_segment(v))
        } else {
            (rest, Segment::Any)
        };

        // No `:` or `/` delimiter means it's a function name pattern
        let (namespace, rest) = if let Some((ns, r)) = rest.split_once(':') {
            (parse_segment(ns), r)
        } else {
            return Ok(Self {
                namespace: Segment::Any,
                package: Segment::Any,
                interface: Segment::Any,
                version,
                function: parse_segment(rest),
            });
        };

        let (package, interface) = if let Some((p, i)) = rest.split_once('/') {
            (parse_segment(p), parse_segment(i))
        } else {
            (parse_segment(rest), Segment::Any)
        };

        Ok(Self {
            namespace,
            package,
            interface,
            version,
            function,
        })
    }

    /// Check if this pattern matches a function, optionally within an interface.
    ///
    /// - For interface-bound functions, pass `Some("namespace:package/interface@version")`.
    /// - For direct (world-level) functions, pass `None`.
    ///
    /// A pattern with interface qualifiers (namespace, package, interface, or version
    /// set to something other than `Any`) will never match a direct function.
    pub fn matches(&self, interface_name: Option<&str>, func_name: &str) -> bool {
        match interface_name {
            Some(full_name) => {
                self.matches_interface(full_name) && self.function.matches(func_name)
            }
            None => {
                self.namespace.is_any()
                    && self.package.is_any()
                    && self.interface.is_any()
                    && self.version.is_any()
                    && self.function.matches(func_name)
            }
        }
    }

    // Check if the interface qualifiers match a fully-qualified interface name.
    fn matches_interface(&self, full_name: &str) -> bool {
        let Some((ns, rest)) = full_name.split_once(':') else {
            return false;
        };
        let Some((pkg, rest)) = rest.split_once('/') else {
            return false;
        };
        let (iface, ver) = if let Some((i, v)) = rest.split_once('@') {
            (i, Some(v))
        } else {
            (rest, None)
        };

        if !self.namespace.matches(ns)
            || !self.package.matches(pkg)
            || !self.interface.matches(iface)
        {
            return false;
        }

        match (&self.version, ver) {
            (Segment::Any | Segment::Wildcard, _) => true,
            (Segment::Literal(pat_ver), Some(actual_ver)) => pat_ver == actual_ver,
            (Segment::Literal(_), None) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Parser tests ===

    #[test]
    fn parse_full_pattern() {
        let p = Pattern::parse("test:example/handler@0.2.0#handle").unwrap();
        assert_eq!(p.namespace, Segment::Literal("test".into()));
        assert_eq!(p.package, Segment::Literal("example".into()));
        assert_eq!(p.interface, Segment::Literal("handler".into()));
        assert_eq!(p.version, Segment::Literal("0.2.0".into()));
        assert_eq!(p.function, Segment::Literal("handle".into()));
    }

    #[test]
    fn parse_interface_only() {
        let p = Pattern::parse("test:example/handler").unwrap();
        assert_eq!(p.namespace, Segment::Literal("test".into()));
        assert_eq!(p.package, Segment::Literal("example".into()));
        assert_eq!(p.interface, Segment::Literal("handler".into()));
        assert_eq!(p.version, Segment::Any);
        assert_eq!(p.function, Segment::Any);
    }

    #[test]
    fn parse_wildcard_interface() {
        let p = Pattern::parse("test:example/*").unwrap();
        assert_eq!(p.interface, Segment::Wildcard);
    }

    #[test]
    fn parse_namespace_only() {
        let p = Pattern::parse("test:*").unwrap();
        assert_eq!(p.namespace, Segment::Literal("test".into()));
        assert_eq!(p.package, Segment::Wildcard);
        assert_eq!(p.interface, Segment::Any);
    }

    #[test]
    fn parse_match_all() {
        let p = Pattern::parse("*").unwrap();
        assert_eq!(p.namespace, Segment::Any);
        assert_eq!(p.function, Segment::Wildcard);
    }

    #[test]
    fn parse_direct_function_name() {
        let p = Pattern::parse("add").unwrap();
        assert_eq!(p.namespace, Segment::Any);
        assert_eq!(p.package, Segment::Any);
        assert_eq!(p.interface, Segment::Any);
        assert_eq!(p.version, Segment::Any);
        assert_eq!(p.function, Segment::Literal("add".into()));
    }

    #[test]
    fn parse_direct_function_glob() {
        let p = Pattern::parse("say-*").unwrap();
        assert_eq!(p.namespace, Segment::Any);
        assert_eq!(p.function, Segment::Literal("say-*".into()));
    }

    // === Matching tests — interface-bound functions ===

    #[test]
    fn matches_interface_exact() {
        let p = Pattern::parse("test:example/handler@0.2.0").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(!p.matches(Some("test:example/handler@0.3.0"), "handle"));
        assert!(!p.matches(Some("test:example/types@0.2.0"), "handle"));
    }

    #[test]
    fn matches_interface_no_version() {
        let p = Pattern::parse("test:example/handler").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(p.matches(Some("test:example/handler@0.3.0"), "handle"));
        assert!(p.matches(Some("test:example/handler"), "handle"));
    }

    #[test]
    fn matches_interface_wildcard() {
        let p = Pattern::parse("test:example/*").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(p.matches(Some("test:example/types@0.2.0"), "get"));
        assert!(!p.matches(Some("test:foo/bar@0.2.0"), "bar"));
    }

    #[test]
    fn matches_interface_function_filter() {
        let p = Pattern::parse("test:example/handler#handle").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(!p.matches(Some("test:example/handler@0.2.0"), "other"));
    }

    #[test]
    fn matches_everything() {
        let p = Pattern::parse("*").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(p.matches(Some("foo:bar/baz"), "anything"));
        assert!(p.matches(None, "add"));
    }

    #[test]
    fn matches_function_name_in_any_interface() {
        let p = Pattern::parse("handle").unwrap();
        assert!(p.matches(Some("test:example/handler@0.2.0"), "handle"));
        assert!(!p.matches(Some("test:example/handler@0.2.0"), "other"));
        assert!(p.matches(Some("foo:bar/baz"), "handle"));
    }

    // === Matching tests — direct functions ===

    #[test]
    fn matches_direct_function_exact() {
        let p = Pattern::parse("add").unwrap();
        assert!(p.matches(None, "add"));
        assert!(!p.matches(None, "subtract"));
    }

    #[test]
    fn matches_direct_function_wildcard_all() {
        let p = Pattern::parse("*").unwrap();
        assert!(p.matches(None, "add"));
        assert!(p.matches(None, "subtract"));
    }

    #[test]
    fn matches_direct_function_glob() {
        let p = Pattern::parse("say-*").unwrap();
        assert!(p.matches(None, "say-hello"));
        assert!(p.matches(None, "say-goodbye"));
        assert!(!p.matches(None, "greet"));
    }

    #[test]
    fn interface_pattern_does_not_match_direct_function() {
        let p = Pattern::parse("test:example/handler").unwrap();
        assert!(!p.matches(None, "handle"));

        let p = Pattern::parse("test:*").unwrap();
        assert!(!p.matches(None, "anything"));

        let p = Pattern::parse("test:example/handler#handle").unwrap();
        assert!(!p.matches(None, "handle"));
    }

    // === Glob matching tests ===

    #[test]
    fn glob_basic() {
        assert!(glob_match("say-*", "say-hello"));
        assert!(glob_match("say-*", "say-goodbye"));
        assert!(glob_match("say-*", "say-"));
        assert!(!glob_match("say-*", "greet"));
    }

    #[test]
    fn glob_middle() {
        assert!(glob_match("get-*-value", "get-my-value"));
        assert!(glob_match("get-*-value", "get-your-value"));
        assert!(!glob_match("get-*-value", "get-my-thing"));
    }

    #[test]
    fn glob_multiple_stars() {
        assert!(glob_match("*-*", "say-hello"));
        assert!(glob_match("*-*", "a-b"));
        assert!(!glob_match("*-*", "nohyphen"));
    }
}
