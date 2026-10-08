use std::path::PathBuf;

use davi_core::collection::{Collection, Node};
use davi_core::env::VarScope;
use davi_core::model::HttpMethod;

fn sample() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample-collection")
}

#[test]
fn loads_sample_collection_tree() {
    let c = Collection::open(sample()).unwrap();
    assert!(c.issues.is_empty(), "{:?}", c.issues);
    assert_eq!(c.name, "Davi Sample");

    // Folders first, then requests; each ordered by `seq`.
    let top: Vec<_> = c.root.children.iter().map(Node::name).collect();
    assert_eq!(top, ["Users", "Ping"]);

    let all: Vec<_> = c.requests().map(|r| (r.method, r.name.as_str())).collect();
    assert_eq!(
        all,
        [
            (HttpMethod::Post, "Create User"),
            (HttpMethod::Get, "Get User"),
            (HttpMethod::Put, "Upload Avatar"),
            (HttpMethod::Get, "Ping"),
        ]
    );

    let local = c.environment("Local").unwrap();
    let scope = VarScope::new().with_layer(local.enabled_vars());
    assert_eq!(
        scope.interpolate("{{baseUrl}}/get"),
        "https://httpbin.org/get"
    );
}
