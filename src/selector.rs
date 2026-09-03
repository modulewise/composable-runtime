//! Label/field selector parsing and matching.

use anyhow::Result;
use std::collections::HashMap;

/// Comparison operator for a selector condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    Equals(String),
    NotEquals(String),
    In(Vec<String>),
    NotIn(Vec<String>),
    Contains(String),
    NotContains(String),
    Exists,
    DoesNotExist,
}

/// A single condition within a selector.
#[derive(Debug, Clone)]
pub struct Condition {
    pub key: String,
    pub operator: Operator,
}

/// Matches against a flattened string-to-string map.
/// All conditions must match (AND semantics).
#[derive(Debug, Clone)]
pub struct Selector {
    pub conditions: Vec<Condition>,
}

impl Selector {
    /// Parse a selector string.
    ///
    /// Comma-separated conditions with AND semantics. Supported expressions:
    /// - equality/inequality: `key=val`, `key!=val`
    /// - set membership: `key in (a,b,c)`, `key notin (a,b,c)`
    /// - substring or list element match: `key contains val`, `key notcontains val`
    /// - key exists/does-not-exist: `key`, `!key`
    pub fn parse(s: &str) -> Result<Self> {
        let mut conditions = Vec::new();
        for part in split_conditions(s) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            conditions.push(parse_condition(part)?);
        }
        if conditions.is_empty() {
            anyhow::bail!("empty selector");
        }
        Ok(Self { conditions })
    }

    pub fn matches(&self, properties: &HashMap<String, Option<String>>) -> bool {
        self.conditions.iter().all(|c| c.matches(properties))
    }
}

impl Condition {
    fn matches(&self, properties: &HashMap<String, Option<String>>) -> bool {
        match &self.operator {
            Operator::Equals(expected) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| v == expected)),
            Operator::NotEquals(expected) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| v != expected)),
            Operator::In(values) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| values.iter().any(|e| v == e))),
            Operator::NotIn(values) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| values.iter().all(|e| v != e))),
            Operator::Contains(needle) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| contains_match(v, needle))),
            Operator::NotContains(needle) => properties
                .get(&self.key)
                .is_some_and(|v| v.as_ref().is_some_and(|v| !contains_match(v, needle))),
            Operator::Exists => properties.contains_key(&self.key),
            Operator::DoesNotExist => !properties.contains_key(&self.key),
        }
    }
}

// List values are bracketed: "[a,b,c]". Scalars have no brackets.
fn contains_match(value: &str, needle: &str) -> bool {
    if let Some(inner) = value.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner.split(',').any(|elem| elem == needle)
    } else {
        value.contains(needle)
    }
}

// Split on commas that are not inside parentheses (to preserve `in (a,b,c)`).
fn split_conditions(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0;

    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn parse_condition(s: &str) -> Result<Condition> {
    // Try != before = to avoid matching the wrong operator
    if let Some((key, val)) = s.split_once("!=") {
        return Ok(Condition {
            key: key.trim().to_string(),
            operator: Operator::NotEquals(val.trim().to_string()),
        });
    }

    if let Some((key, val)) = s.split_once('=') {
        return Ok(Condition {
            key: key.trim().to_string(),
            operator: Operator::Equals(val.trim().to_string()),
        });
    }

    // Keyword operators: "key in (...)", "key notin (...)", "key contains val", "key notcontains val"
    let parts: Vec<&str> = s.splitn(3, ' ').collect();
    if parts.len() >= 2 {
        let key = parts[0].trim();
        match parts[1].trim() {
            "in" => {
                let rest = parts.get(2).unwrap_or(&"").trim();
                let values = parse_value_list(rest)?;
                return Ok(Condition {
                    key: key.to_string(),
                    operator: Operator::In(values),
                });
            }
            "notin" => {
                let rest = parts.get(2).unwrap_or(&"").trim();
                let values = parse_value_list(rest)?;
                return Ok(Condition {
                    key: key.to_string(),
                    operator: Operator::NotIn(values),
                });
            }
            "contains" => {
                let val = parts.get(2).unwrap_or(&"").trim();
                if val.is_empty() {
                    anyhow::bail!("missing value for 'contains' in: {s}");
                }
                return Ok(Condition {
                    key: key.to_string(),
                    operator: Operator::Contains(val.to_string()),
                });
            }
            "notcontains" => {
                let val = parts.get(2).unwrap_or(&"").trim();
                if val.is_empty() {
                    anyhow::bail!("missing value for 'notcontains' in: {s}");
                }
                return Ok(Condition {
                    key: key.to_string(),
                    operator: Operator::NotContains(val.to_string()),
                });
            }
            _ => {}
        }
    }

    // Existence: "!key" or "key"
    let trimmed = s.trim();
    if let Some(key) = trimmed.strip_prefix('!') {
        Ok(Condition {
            key: key.to_string(),
            operator: Operator::DoesNotExist,
        })
    } else {
        Ok(Condition {
            key: trimmed.to_string(),
            operator: Operator::Exists,
        })
    }
}

fn parse_value_list(s: &str) -> Result<Vec<String>> {
    let inner = s
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| anyhow::anyhow!("expected parenthesized list like (a,b,c), got: {s}"))?;
    if inner.trim().is_empty() {
        anyhow::bail!("empty value list in: {s}");
    }
    Ok(inner.split(',').map(|v| v.trim().to_string()).collect())
}
