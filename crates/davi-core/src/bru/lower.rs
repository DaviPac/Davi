//! Conversion between the generic block AST and the typed [`HttpRequest`].

use super::ast::{Block, BruFile};
use crate::error::LowerError;
use crate::model::{
    ApiKeyPlacement, Auth, BodyMode, HttpMethod, HttpRequest, KeyValue, Meta, RequestKind,
};

fn find<'a>(entries: &'a [KeyValue], key: &str) -> Option<&'a str> {
    entries
        .iter()
        .find(|e| e.name == key)
        .map(|e| e.value.as_str())
}

fn dict_or_empty(block: &Block) -> &[KeyValue] {
    block.body.as_dict().unwrap_or_default()
}

fn text(block: &Block) -> Option<String> {
    block.body.as_text().map(str::to_owned)
}

impl HttpRequest {
    /// Lower a parsed `.bru` file into a typed request.
    pub fn from_bru(file: &BruFile) -> Result<Self, LowerError> {
        let mut req = HttpRequest::default();
        let mut method_block = None;
        let mut auth_blocks: Vec<&Block> = Vec::new();

        for block in &file.blocks {
            let name = block.name.as_str();
            if let Some(method) = HttpMethod::from_bru_block(name)
                && method_block.is_none()
            {
                req.method = method;
                method_block = Some(block);
                continue;
            }
            match name {
                "meta" => req.meta = lower_meta(dict_or_empty(block)),
                "params:query" => req.query_params = dict_or_empty(block).to_vec(),
                "params:path" => req.path_params = dict_or_empty(block).to_vec(),
                "headers" => req.headers = dict_or_empty(block).to_vec(),
                "body:json" => req.body.json = text(block),
                "body:text" => req.body.text = text(block),
                "body:xml" => req.body.xml = text(block),
                "body:sparql" => req.body.sparql = text(block),
                "body:graphql" => req.body.graphql = text(block),
                "body:graphql:vars" => req.body.graphql_vars = text(block),
                "body:form-urlencoded" => req.body.form_urlencoded = dict_or_empty(block).to_vec(),
                "body:multipart-form" => req.body.multipart_form = dict_or_empty(block).to_vec(),
                "vars:pre-request" => req.vars.pre_request = dict_or_empty(block).to_vec(),
                "vars:post-response" => req.vars.post_response = dict_or_empty(block).to_vec(),
                "assert" => req.assertions = dict_or_empty(block).to_vec(),
                "script:pre-request" => req.scripts.pre_request = text(block),
                "script:post-response" => req.scripts.post_response = text(block),
                "tests" => req.tests = text(block),
                "docs" => req.docs = text(block),
                n if n.starts_with("auth:") => auth_blocks.push(block),
                _ => req.extra_blocks.push(block.clone()),
            }
        }

        let method_block = method_block.ok_or(LowerError::MissingMethodBlock)?;
        let http = dict_or_empty(method_block);
        req.url = find(http, "url").unwrap_or_default().to_owned();
        req.body.mode = BodyMode::parse(find(http, "body").unwrap_or("none"));

        let auth_mode = find(http, "auth").unwrap_or("none");
        let auth_block = format!("auth:{auth_mode}");
        let mut active = None;
        for block in auth_blocks {
            if active.is_none() && block.name == auth_block {
                active = Some(block);
            } else {
                req.extra_blocks.push(block.clone());
            }
        }
        req.auth = lower_auth(auth_mode, active, &mut req.extra_blocks);

        Ok(req)
    }

    /// Raise this request back into a `.bru` block AST, in Bruno's canonical
    /// block order.
    pub fn to_bru(&self) -> BruFile {
        let mut blocks = Vec::with_capacity(8 + self.extra_blocks.len());

        let mut meta = vec![
            KeyValue::new("name", &self.meta.name),
            KeyValue::new("type", self.meta.kind.as_str()),
        ];
        if let Some(seq) = self.meta.seq {
            meta.push(KeyValue::new("seq", seq.to_string()));
        }
        meta.extend(self.meta.extra.iter().cloned());
        blocks.push(Block::dict("meta", meta));

        blocks.push(Block::dict(
            self.method.bru_block(),
            vec![
                KeyValue::new("url", &self.url),
                KeyValue::new("body", self.body.mode.as_str()),
                KeyValue::new("auth", self.auth.mode()),
            ],
        ));

        push_dict(&mut blocks, "params:query", &self.query_params);
        push_dict(&mut blocks, "params:path", &self.path_params);
        push_dict(&mut blocks, "headers", &self.headers);

        match &self.auth {
            Auth::Bearer { token } => blocks.push(Block::dict(
                "auth:bearer",
                vec![KeyValue::new("token", token)],
            )),
            Auth::Basic { username, password } => blocks.push(Block::dict(
                "auth:basic",
                vec![
                    KeyValue::new("username", username),
                    KeyValue::new("password", password),
                ],
            )),
            Auth::ApiKey {
                key,
                value,
                placement,
            } => blocks.push(Block::dict(
                "auth:apikey",
                vec![
                    KeyValue::new("key", key),
                    KeyValue::new("value", value),
                    KeyValue::new("placement", placement.as_str()),
                ],
            )),
            Auth::None | Auth::Inherit | Auth::Unsupported { .. } => {}
        }

        let b = &self.body;
        push_text(&mut blocks, "body:json", &b.json);
        push_text(&mut blocks, "body:text", &b.text);
        push_text(&mut blocks, "body:xml", &b.xml);
        push_text(&mut blocks, "body:sparql", &b.sparql);
        push_dict(&mut blocks, "body:form-urlencoded", &b.form_urlencoded);
        push_dict(&mut blocks, "body:multipart-form", &b.multipart_form);
        push_text(&mut blocks, "body:graphql", &b.graphql);
        push_text(&mut blocks, "body:graphql:vars", &b.graphql_vars);

        push_dict(&mut blocks, "vars:pre-request", &self.vars.pre_request);
        push_dict(&mut blocks, "vars:post-response", &self.vars.post_response);
        push_dict(&mut blocks, "assert", &self.assertions);
        push_text(&mut blocks, "script:pre-request", &self.scripts.pre_request);
        push_text(
            &mut blocks,
            "script:post-response",
            &self.scripts.post_response,
        );
        push_text(&mut blocks, "tests", &self.tests);

        blocks.extend(self.extra_blocks.iter().cloned());
        push_text(&mut blocks, "docs", &self.docs);

        BruFile { blocks }
    }
}

fn push_dict(blocks: &mut Vec<Block>, name: &str, entries: &[KeyValue]) {
    if !entries.is_empty() {
        blocks.push(Block::dict(name, entries.to_vec()));
    }
}

fn push_text(blocks: &mut Vec<Block>, name: &str, text: &Option<String>) {
    if let Some(text) = text {
        blocks.push(Block::text(name, text.clone()));
    }
}

fn lower_meta(entries: &[KeyValue]) -> Meta {
    let mut meta = Meta::default();
    for e in entries {
        match e.name.as_str() {
            "name" => meta.name = e.value.clone(),
            "type" => meta.kind = RequestKind::parse(&e.value),
            "seq" => meta.seq = e.value.trim().parse().ok(),
            _ => meta.extra.push(e.clone()),
        }
    }
    meta
}

fn lower_auth(mode: &str, block: Option<&Block>, extra: &mut Vec<Block>) -> Auth {
    let entries = block.map(dict_or_empty).unwrap_or_default();
    let get = |k: &str| find(entries, k).unwrap_or_default().to_owned();
    match mode {
        "none" => Auth::None,
        "inherit" => Auth::Inherit,
        "bearer" => Auth::Bearer {
            token: get("token"),
        },
        "basic" => Auth::Basic {
            username: get("username"),
            password: get("password"),
        },
        "apikey" => Auth::ApiKey {
            key: get("key"),
            value: get("value"),
            placement: match find(entries, "placement") {
                Some("queryparams") => ApiKeyPlacement::QueryParams,
                _ => ApiKeyPlacement::Header,
            },
        },
        other => {
            // Keep the block of an auth mode we can't execute yet.
            if let Some(block) = block {
                extra.push(block.clone());
            }
            Auth::Unsupported {
                mode: other.to_owned(),
            }
        }
    }
}
