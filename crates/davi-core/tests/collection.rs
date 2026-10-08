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

#[test]
fn scaffolds_collections_folders_and_requests() {
    use davi_core::collection::{create_collection, create_folder, create_request, load_request};

    let tmp = std::env::temp_dir().join(format!("davi-scaffold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let root = create_collection(&tmp, "My API").unwrap();
    assert_eq!(root.file_name().unwrap(), "My API");
    // A second collection with the same name gets a unique directory.
    assert_eq!(
        create_collection(&tmp, "My API")
            .unwrap()
            .file_name()
            .unwrap(),
        "My API-2"
    );

    let first = create_request(&root, "List: users?").unwrap();
    assert_eq!(first.file_name().unwrap(), "List- users-.bru");
    let second = create_request(&root, "Create user").unwrap();
    let folder = create_folder(&root, "Admin").unwrap();
    let nested = create_request(&folder, "Ban user").unwrap();

    assert_eq!(load_request(&first).unwrap().meta.seq, Some(1));
    assert_eq!(load_request(&second).unwrap().meta.name, "Create user");
    assert_eq!(load_request(&second).unwrap().meta.seq, Some(2));
    assert_eq!(load_request(&nested).unwrap().meta.seq, Some(1));

    let c = Collection::open(&root).unwrap();
    assert!(c.issues.is_empty(), "{:?}", c.issues);
    assert_eq!(c.name, "My API");
    let names: Vec<_> = c.requests().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Ban user", "List: users?", "Create user"]);

    std::fs::remove_dir_all(&tmp).unwrap();
}
