//! Resolving a [`HttpRequest`] + [`VarScope`] into a concrete, fully
//! interpolated [`PreparedRequest`].
//!
//! This step is pure (no I/O, no runtime), which makes it cheap to unit-test
//! and lets the UI show the exact request that will be sent.

use std::path::PathBuf;

use davi_core::env::VarScope;
use davi_core::model::{ApiKeyPlacement, Auth, BodyMode, HttpMethod, HttpRequest, KeyValue};
use reqwest::Url;

use crate::error::NetError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub method: HttpMethod,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub basic_auth: Option<(String, String)>,
    pub body: PreparedBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedBody {
    None,
    /// Raw bytes (json/text/xml/sparql/graphql), Content-Type already set.
    Raw(String),
    Form(Vec<(String, String)>),
    Multipart(Vec<MultipartPart>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPart {
    pub name: String,
    pub value: MultipartValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultipartValue {
    Text(String),
    File(PathBuf),
}

impl PreparedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn set_default_header(&mut self, name: &str, value: &str) {
        if self.header(name).is_none() {
            self.headers.push((name.to_owned(), value.to_owned()));
        }
    }
}

/// Interpolate variables and resolve auth/body into a sendable request.
pub fn prepare(request: &HttpRequest, vars: &VarScope) -> Result<PreparedRequest, NetError> {
    // Request-level `vars:pre-request` take precedence over everything else.
    let local;
    let vars = if request.vars.pre_request.iter().any(|v| v.enabled) {
        local = layered(request, vars);
        &local
    } else {
        vars
    };
    let interp = |s: &str| vars.interpolate(s).into_owned();
    let pairs = |kvs: &[KeyValue]| -> Vec<(String, String)> {
        kvs.iter()
            .filter(|kv| kv.enabled)
            .map(|kv| (interp(&kv.name), interp(&kv.value)))
            .collect()
    };

    let raw_url = substitute_path_params(&interp(&request.url), &pairs(&request.path_params));
    let mut url = parse_url(&raw_url)?;

    let mut prepared = PreparedRequest {
        method: request.method,
        url: url.clone(),
        headers: pairs(&request.headers),
        basic_auth: None,
        body: PreparedBody::None,
    };

    match &request.auth {
        Auth::Bearer { token } => {
            prepared.set_default_header("Authorization", &format!("Bearer {}", interp(token)))
        }
        Auth::Basic { username, password } => {
            prepared.basic_auth = Some((interp(username), interp(password)))
        }
        Auth::ApiKey {
            key,
            value,
            placement,
        } => match placement {
            ApiKeyPlacement::Header => prepared.headers.push((interp(key), interp(value))),
            ApiKeyPlacement::QueryParams => {
                url.query_pairs_mut()
                    .append_pair(&interp(key), &interp(value));
                prepared.url = url;
            }
        },
        // TODO(collection-auth): resolve `inherit` from folder/collection.bru.
        Auth::None | Auth::Inherit | Auth::Unsupported { .. } => {}
    }

    let body = &request.body;
    let raw = |text: &Option<String>, content_type: &str, prepared: &mut PreparedRequest| {
        prepared.set_default_header("Content-Type", content_type);
        PreparedBody::Raw(interp(text.as_deref().unwrap_or_default()))
    };
    prepared.body = match &body.mode {
        BodyMode::None | BodyMode::Other(_) => PreparedBody::None,
        BodyMode::Json => raw(&body.json, "application/json", &mut prepared),
        BodyMode::Text => raw(&body.text, "text/plain", &mut prepared),
        BodyMode::Xml => raw(&body.xml, "application/xml", &mut prepared),
        BodyMode::Sparql => raw(&body.sparql, "application/sparql-query", &mut prepared),
        BodyMode::Graphql => {
            prepared.set_default_header("Content-Type", "application/json");
            PreparedBody::Raw(graphql_payload(
                &interp(body.graphql.as_deref().unwrap_or_default()),
                &interp(body.graphql_vars.as_deref().unwrap_or_default()),
            )?)
        }
        BodyMode::FormUrlEncoded => PreparedBody::Form(pairs(&body.form_urlencoded)),
        BodyMode::MultipartForm => PreparedBody::Multipart(
            pairs(&body.multipart_form)
                .into_iter()
                .map(|(name, value)| MultipartPart {
                    name,
                    value: parse_multipart_value(value),
                })
                .collect(),
        ),
    };

    Ok(prepared)
}

fn layered(request: &HttpRequest, base: &VarScope) -> VarScope {
    // Request vars may themselves reference environment variables.
    let resolved: Vec<(String, String)> = request
        .vars
        .pre_request
        .iter()
        .filter(|v| v.enabled)
        .map(|v| (v.name.clone(), base.interpolate(&v.value).into_owned()))
        .collect();
    let mut scope = VarScope::new().with_layer(resolved);
    scope.extend_from(base);
    scope
}

/// Bruno prefixes scheme-less URLs with `http://`.
fn parse_url(raw: &str) -> Result<Url, NetError> {
    let trimmed = raw.trim();
    let candidate = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    Url::parse(&candidate).map_err(|e| NetError::InvalidUrl {
        url: raw.to_owned(),
        reason: e.to_string(),
    })
}

/// Replace `/:name` path segments with their values.
fn substitute_path_params(url: &str, params: &[(String, String)]) -> String {
    if params.is_empty() || !url.contains("/:") {
        return url.to_owned();
    }
    let (path_part, suffix) = match url.find(['?', '#']) {
        Some(i) => url.split_at(i),
        None => (url, ""),
    };
    let mut out = String::with_capacity(url.len());
    for (i, segment) in path_part.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        let replacement = segment
            .strip_prefix(':')
            .and_then(|name| params.iter().find(|(k, _)| k == name))
            .map(|(_, v)| v.as_str());
        out.push_str(replacement.unwrap_or(segment));
    }
    out.push_str(suffix);
    out
}

/// `@file(path)` → file part, anything else → text part.
fn parse_multipart_value(value: String) -> MultipartValue {
    match value
        .strip_prefix("@file(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        Some(path) => MultipartValue::File(PathBuf::from(path)),
        None => MultipartValue::Text(value),
    }
}

fn graphql_payload(query: &str, variables: &str) -> Result<String, NetError> {
    let variables = if variables.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(variables).map_err(|e| NetError::InvalidBody {
            reason: format!("GraphQL variables are not valid JSON: {e}"),
        })?
    };
    Ok(serde_json::json!({ "query": query, "variables": variables }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use davi_core::bru::parse_request;

    fn vars() -> VarScope {
        VarScope::new().with_layer([("baseUrl", "api.test"), ("token", "t0k"), ("id", "7")])
    }

    #[test]
    fn interpolates_url_headers_and_path_params() {
        let req = parse_request(
            "get {\n  url: {{baseUrl}}/users/:id/posts?x=1\n  auth: bearer\n}\n\nparams:path {\n  id: {{id}}\n}\n\nheaders {\n  X-A: {{token}}\n  ~X-Off: no\n}\n\nauth:bearer {\n  token: {{token}}\n}\n",
        )
        .unwrap();
        let p = prepare(&req, &vars()).unwrap();
        assert_eq!(p.url.as_str(), "http://api.test/users/7/posts?x=1");
        assert_eq!(p.header("x-a"), Some("t0k"));
        assert_eq!(p.header("X-Off"), None);
        assert_eq!(p.header("authorization"), Some("Bearer t0k"));
    }

    #[test]
    fn json_body_gets_default_content_type_and_request_vars_win() {
        let req = parse_request(
            "post {\n  url: http://h\n  body: json\n}\n\nheaders {\n  content-type: application/vnd+json\n}\n\nbody:json {\n  {\"id\": {{id}}}\n}\n\nvars:pre-request {\n  id: 99\n}\n",
        )
        .unwrap();
        let p = prepare(&req, &vars()).unwrap();
        assert_eq!(p.body, PreparedBody::Raw("{\"id\": 99}".into()));
        assert_eq!(p.header("Content-Type"), Some("application/vnd+json"));
        assert_eq!(p.headers.len(), 1);
    }

    #[test]
    fn api_key_query_and_multipart_files() {
        let req = parse_request(
            "post {\n  url: http://h/u\n  body: multipartForm\n  auth: apikey\n}\n\nauth:apikey {\n  key: k\n  value: {{token}}\n  placement: queryparams\n}\n\nbody:multipart-form {\n  a: b\n  f: @file(/tmp/x.png)\n}\n",
        )
        .unwrap();
        let p = prepare(&req, &vars()).unwrap();
        assert_eq!(p.url.as_str(), "http://h/u?k=t0k");
        assert_eq!(
            p.body,
            PreparedBody::Multipart(vec![
                MultipartPart {
                    name: "a".into(),
                    value: MultipartValue::Text("b".into())
                },
                MultipartPart {
                    name: "f".into(),
                    value: MultipartValue::File("/tmp/x.png".into())
                },
            ])
        );
    }

    #[test]
    fn invalid_url_is_reported() {
        let req = parse_request("get {\n  url: http://exa mple\n}\n").unwrap();
        assert!(matches!(
            prepare(&req, &vars()),
            Err(NetError::InvalidUrl { .. })
        ));
    }
}
