//! Environments (`environments/*.bru`) and `{{variable}}` interpolation.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::bru::{self, Block, BruFile};
use crate::error::CoreError;
use crate::model::KeyValue;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
    pub enabled: bool,
    /// Listed in `vars:secret`; its value is never written to disk.
    pub secret: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Environment {
    pub name: String,
    pub variables: Vec<EnvVariable>,
}

impl Environment {
    /// ```text
    /// vars {
    ///   baseUrl: http://localhost:3000
    ///   ~legacy: http://old
    /// }
    /// vars:secret [
    ///   token
    /// ]
    /// ```
    pub fn from_bru(name: impl Into<String>, file: &BruFile) -> Self {
        let mut variables: Vec<EnvVariable> = file
            .dict("vars")
            .unwrap_or_default()
            .iter()
            .map(|kv| EnvVariable {
                name: kv.name.clone(),
                value: kv.value.clone(),
                enabled: kv.enabled,
                secret: false,
            })
            .collect();

        if let Some(secrets) = file.block("vars:secret").and_then(|b| b.body.as_list()) {
            for raw in secrets {
                let (enabled, name) = match raw.strip_prefix('~') {
                    Some(n) => (false, n),
                    None => (true, raw.as_str()),
                };
                variables.push(EnvVariable {
                    name: name.to_owned(),
                    value: String::new(),
                    enabled,
                    secret: true,
                });
            }
        }

        Self {
            name: name.into(),
            variables,
        }
    }

    pub fn to_bru(&self) -> BruFile {
        let (secrets, plain): (Vec<_>, Vec<_>) = self.variables.iter().partition(|v| v.secret);
        let mut blocks = vec![Block::dict(
            "vars",
            plain
                .iter()
                .map(|v| KeyValue {
                    name: v.name.clone(),
                    value: v.value.clone(),
                    enabled: v.enabled,
                })
                .collect(),
        )];
        if !secrets.is_empty() {
            blocks.push(Block::list(
                "vars:secret",
                secrets
                    .iter()
                    .map(|v| {
                        if v.enabled {
                            v.name.clone()
                        } else {
                            format!("~{}", v.name)
                        }
                    })
                    .collect(),
            ));
        }
        BruFile { blocks }
    }

    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let source = std::fs::read_to_string(path).map_err(|e| CoreError::io(path, e))?;
        let file = bru::parse(&source).map_err(|e| CoreError::from(e).in_file(path))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::from_bru(name, &file))
    }

    /// Enabled variables as `(name, value)` pairs.
    pub fn enabled_vars(&self) -> impl Iterator<Item = (&str, &str)> {
        self.variables
            .iter()
            .filter(|v| v.enabled)
            .map(|v| (v.name.as_str(), v.value.as_str()))
    }
}

/// Layered variable lookup. Earlier layers win, matching Bruno's precedence:
/// runtime > request > folder > environment > collection > `process.env`.
#[derive(Debug, Clone, Default)]
pub struct VarScope {
    layers: Vec<HashMap<String, String>>,
}

/// Nested `{{a}}` → `{{b}}` → value chains are resolved up to this depth,
/// which also guards against reference cycles.
const MAX_DEPTH: usize = 8;

impl VarScope {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a layer with *lower* priority than every existing one.
    pub fn push_layer<K, V>(&mut self, vars: impl IntoIterator<Item = (K, V)>) -> &mut Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.layers.push(
            vars.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        );
        self
    }

    pub fn with_layer<K, V>(mut self, vars: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.push_layer(vars);
        self
    }

    /// Append all layers of `other` with lower priority than ours.
    pub fn extend_from(&mut self, other: &VarScope) -> &mut Self {
        self.layers.extend(other.layers.iter().cloned());
        self
    }

    /// Raw (un-interpolated) value of a variable.
    pub fn get(&self, name: &str) -> Option<Cow<'_, str>> {
        if let Some(value) = self.layers.iter().find_map(|l| l.get(name)) {
            return Some(Cow::Borrowed(value));
        }
        if let Some(key) = name.strip_prefix("process.env.") {
            return std::env::var(key).ok().map(Cow::Owned);
        }
        dynamic_var(name).map(Cow::Owned)
    }

    /// Replace every `{{name}}` in `input`. Unknown variables are left as-is,
    /// like Bruno does, so they stay visible in the sent request.
    ///
    /// Returns `Cow::Borrowed` when there is nothing to replace, which is the
    /// common case and avoids an allocation per header/param.
    pub fn interpolate<'a>(&self, input: &'a str) -> Cow<'a, str> {
        self.interpolate_depth(input, 0)
    }

    fn interpolate_depth<'a>(&self, input: &'a str, depth: usize) -> Cow<'a, str> {
        let Some(first) = input.find("{{") else {
            return Cow::Borrowed(input);
        };

        let mut out = String::with_capacity(input.len() + 16);
        out.push_str(&input[..first]);
        let mut rest = &input[first..];

        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after_open = &rest[start + 2..];
            let Some(end) = after_open.find("}}") else {
                // Unbalanced: copy the remainder verbatim.
                out.push_str(&rest[start..]);
                return Cow::Owned(out);
            };
            let name = after_open[..end].trim();
            let placeholder = &rest[start..start + 2 + end + 2];
            match self.get(name) {
                Some(value) if depth < MAX_DEPTH => {
                    out.push_str(&self.interpolate_depth(&value, depth + 1))
                }
                _ => out.push_str(placeholder),
            }
            rest = &after_open[end + 2..];
        }
        out.push_str(rest);
        Cow::Owned(out)
    }
}

/// Bruno-style dynamic variables that need no extra dependencies.
fn dynamic_var(name: &str) -> Option<String> {
    let now = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
    };
    match name {
        "$timestamp" => Some(now().as_secs().to_string()),
        "$timestampMs" => Some(now().as_millis().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> VarScope {
        VarScope::new()
            .with_layer([("token", "req-token")])
            .with_layer([
                ("baseUrl", "https://{{host}}/api"),
                ("host", "example.com"),
                ("token", "env-token"),
                ("loop", "{{loop}}"),
            ])
    }

    #[test]
    fn no_placeholders_borrows() {
        assert!(matches!(
            scope().interpolate("plain"),
            Cow::Borrowed("plain")
        ));
    }

    #[test]
    fn resolves_nested_and_respects_precedence() {
        let s = scope();
        assert_eq!(
            s.interpolate("{{baseUrl}}/users"),
            "https://example.com/api/users"
        );
        assert_eq!(s.interpolate("Bearer {{ token }}"), "Bearer req-token");
    }

    #[test]
    fn leaves_unknown_and_unbalanced_untouched() {
        let s = scope();
        assert_eq!(s.interpolate("{{nope}}-{{host}}"), "{{nope}}-example.com");
        assert_eq!(s.interpolate("a {{host"), "a {{host");
    }

    #[test]
    fn cycles_terminate() {
        assert_eq!(scope().interpolate("{{loop}}"), "{{loop}}");
    }

    #[test]
    fn environment_round_trip() {
        let src = "vars {\n  baseUrl: http://localhost:3000\n  ~legacy: http://old\n}\n\nvars:secret [\n  token,\n  ~apiKey\n]\n";
        let env = Environment::from_bru("Local", &bru::parse(src).unwrap());
        assert_eq!(env.variables.len(), 4);
        assert!(env.variables[2].secret && env.variables[2].enabled);
        assert!(!env.variables[3].enabled);
        assert_eq!(bru::write(&env.to_bru()), src);
    }
}
