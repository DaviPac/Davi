use super::*;
use crate::model::{ApiKeyPlacement, Auth, BodyMode, HttpMethod, KeyValue};

const CREATE_USER: &str =
    include_str!("../../../../examples/sample-collection/users/create-user.bru");

#[test]
fn lowers_every_section() {
    let req = parse_request(CREATE_USER).unwrap();
    assert_eq!(req.meta.name, "Create User");
    assert_eq!(req.meta.seq, Some(1));
    assert_eq!(req.method, HttpMethod::Post);
    assert_eq!(req.url, "{{baseUrl}}/post");
    assert_eq!(req.body.mode, BodyMode::Json);
    assert_eq!(
        req.body.json.as_deref(),
        Some("{\n  \"id\": {{userId}},\n  \"name\": \"Ada Lovelace\"\n}")
    );
    assert_eq!(
        req.headers,
        vec![
            KeyValue::new("Content-Type", "application/json"),
            KeyValue::disabled("X-Debug", "true"),
        ]
    );
    assert_eq!(
        req.auth,
        Auth::Bearer {
            token: "{{token}}".into()
        }
    );
    assert_eq!(
        req.vars.post_response,
        vec![KeyValue::new("createdId", "res.body.json.id")]
    );
    assert_eq!(req.assertions, vec![KeyValue::new("res.status", "eq 200")]);
    assert_eq!(
        req.docs.as_deref(),
        Some("Creates a user and echoes it back.")
    );
    assert!(req.extra_blocks.is_empty());
}

#[test]
fn round_trips_byte_for_byte() {
    for src in [
        CREATE_USER,
        include_str!("../../../../examples/sample-collection/ping.bru"),
        include_str!("../../../../examples/sample-collection/users/get-user.bru"),
        include_str!("../../../../examples/sample-collection/users/upload-avatar.bru"),
    ] {
        let req = parse_request(src).unwrap();
        assert_eq!(write_request(&req), src);
    }
}

#[test]
fn preserves_unknown_blocks_and_unsupported_auth() {
    let src = "\
meta {
  name: OAuth
  type: http
}

get {
  url: https://example.com
  body: none
  auth: oauth2
}

auth:oauth2 {
  grant_type: client_credentials
  client_id: abc
}

settings {
  encodeUrl: true
}

custom-notes {
  this is free text without a colon
}
";
    let req = parse_request(src).unwrap();
    assert_eq!(
        req.auth,
        Auth::Unsupported {
            mode: "oauth2".into()
        }
    );
    let names: Vec<_> = req.extra_blocks.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["settings", "custom-notes", "auth:oauth2"]);
    assert!(matches!(req.extra_blocks[1].body, BlockBody::Text(_)));

    let again = parse_request(&write_request(&req)).unwrap();
    assert_eq!(again, req);
}

#[test]
fn api_key_auth() {
    let src = "get {\n  url: x\n  auth: apikey\n}\n\nauth:apikey {\n  key: X-Key\n  value: k\n  placement: queryparams\n}\n";
    let req = parse_request(src).unwrap();
    assert_eq!(
        req.auth,
        Auth::ApiKey {
            key: "X-Key".into(),
            value: "k".into(),
            placement: ApiKeyPlacement::QueryParams
        }
    );
}

#[test]
fn multiline_values_and_quoted_keys() {
    let src = "\
vars:pre-request {
  query: '''
    query {
      user
    }
  '''
  \"a:b\": c
}
";
    let file = parse(src).unwrap();
    let vars = file.dict("vars:pre-request").unwrap();
    assert_eq!(vars[0].value, "query {\n  user\n}");
    assert_eq!(vars[1].name, "a:b");
    assert_eq!(write(&file), src);
}

#[test]
fn text_blocks_keep_blank_lines_and_braces() {
    let src = "script:pre-request {\n  if (x) {\n  \n    y();\n  }\n}\n";
    let file = parse(src).unwrap();
    assert_eq!(
        file.text("script:pre-request").unwrap(),
        "if (x) {\n\n  y();\n}"
    );
    assert_eq!(write(&file), src);
}

#[test]
fn accepts_crlf_bom_and_inline_empty_blocks() {
    let src = "\u{feff}meta {\r\n  name: Win\r\n}\r\n\r\nget {\r\n  url: http://a\r\n}\r\n\r\nheaders {}\r\nvars:secret []\r\n";
    let file = parse(src).unwrap();
    assert_eq!(file.blocks.len(), 4);
    assert_eq!(file.dict("meta").unwrap()[0].value, "Win");
    assert_eq!(file.dict("headers"), Some(&[][..]));
}

#[test]
fn reports_positions() {
    let err = parse("meta {\n  name: ok\n}\n\nheaders {\n  no colon here\n}\n").unwrap_err();
    assert_eq!((err.line, err.column), (6, 1));
    assert!(err.message.contains("headers"), "{err}");

    let err = parse("meta {\n  name: ok\n\nget {\n").unwrap_err();
    assert_eq!(err.line, 1);
    assert!(err.message.contains("never closed"), "{err}");

    let err = parse("  ???\n").unwrap_err();
    assert_eq!((err.line, err.column), (1, 3));
}

#[test]
fn missing_method_block() {
    let err = parse_request("meta {\n  name: x\n}\n").unwrap_err();
    assert!(matches!(
        err,
        crate::CoreError::Lower(crate::LowerError::MissingMethodBlock)
    ));
}
