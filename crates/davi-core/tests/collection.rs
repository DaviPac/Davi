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

#[test]
fn environments_and_folder_vars_round_trip_and_layer() {
    use davi_core::collection::{
        create_collection, create_folder, create_request, delete_environment, save_environment,
        save_folder_vars,
    };
    use davi_core::env::{EnvVariable, Environment};
    use davi_core::model::KeyValue;

    let tmp = std::env::temp_dir().join(format!("davi-vars-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let root = create_collection(&tmp, "Vars").unwrap();
    let users = create_folder(&root, "Users").unwrap();
    let admin = create_folder(&users, "Admin").unwrap();
    let top = create_request(&root, "Top").unwrap();
    let nested = create_request(&admin, "Ban").unwrap();

    let var = |name: &str, value: &str| EnvVariable {
        name: name.into(),
        value: value.into(),
        enabled: true,
        secret: false,
    };
    let env = Environment {
        name: "Prod".into(),
        variables: vec![var("host", "prod.example.com"), var("who", "env")],
    };
    save_environment(&root, &env).unwrap();
    save_environment(
        &root,
        &Environment {
            name: "Scratch".into(),
            variables: vec![],
        },
    )
    .unwrap();
    delete_environment(&root, "Scratch").unwrap();

    save_folder_vars(
        &root,
        &root,
        &[
            KeyValue::new("who", "collection"),
            KeyValue::new("only", "c"),
        ],
    )
    .unwrap();
    save_folder_vars(
        &root,
        &users,
        &[
            KeyValue::new("who", "users"),
            KeyValue::new("api", "https://{{host}}/api"),
        ],
    )
    .unwrap();
    save_folder_vars(&root, &admin, &[KeyValue::new("api", "{{api}}/admin")]).unwrap();

    let c = Collection::open(&root).unwrap();
    assert!(c.issues.is_empty(), "{:?}", c.issues);
    assert_eq!(c.environments, std::slice::from_ref(&env));
    // folder.bru keeps its meta; the folder still shows under its name.
    assert_eq!(c.folder(&users).unwrap().name, "Users");
    assert_eq!(c.folder(&admin).unwrap().vars.len(), 1);

    let env = c.environment("Prod");
    let s = c.var_scope(&nested, env);
    assert_eq!(
        s.interpolate("{{api}}"),
        "https://prod.example.com/api/admin"
    );
    assert_eq!(s.interpolate("{{who}} {{only}}"), "users c");

    // Outside the folder: environment beats collection vars.
    let s = c.var_scope(&top, env);
    assert_eq!(s.interpolate("{{who}} {{api}}"), "env {{api}}");
    assert_eq!(c.var_scope(&top, None).interpolate("{{who}}"), "collection");

    // Clearing the vars removes the block but keeps the folder's meta.
    save_folder_vars(&root, &admin, &[]).unwrap();
    let c = Collection::open(&root).unwrap();
    assert!(c.folder(&admin).unwrap().vars.is_empty());
    assert_eq!(c.folder(&admin).unwrap().name, "Admin");

    std::fs::remove_dir_all(&tmp).unwrap();
}
