use telehand_e2e::{Env, json};

#[tokio::test]
async fn write_is_limited_to_project_folders() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let outside = env.dir("outside");
    std::fs::write(outside.join("notes.txt"), "outside notes").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    let wrote = client
        .ok(
            "write",
            json!({"path": "src/new.rs", "content": "fn new() {}\n"}),
        )
        .await;
    assert_eq!(wrote, "Successfully wrote to src/new.rs");
    assert_eq!(
        std::fs::read_to_string(demo.join("src/new.rs")).unwrap(),
        "fn new() {}\n"
    );

    let target = outside.join("evil.txt");
    let denied = client
        .err(
            "write",
            json!({"path": target.to_string_lossy(), "content": "x"}),
        )
        .await;
    assert!(denied.starts_with("Permission denied"), "{denied}");
    assert!(!target.exists());

    let read = client
        .ok(
            "read",
            json!({"path": outside.join("notes.txt").to_string_lossy()}),
        )
        .await;
    assert_eq!(read, "outside notes");

    runner.stop().await;
}

#[tokio::test]
async fn edit_applies_multiple_and_fuzzy_replacements() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let outside = env.dir("outside");
    std::fs::write(
        demo.join("lib.rs"),
        "fn a() {}   \nlet s = \u{201C}hi\u{201D};\nfn b() {}\n",
    )
    .unwrap();
    std::fs::write(outside.join("other.rs"), "fn a() {}\n").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    let edited = client
        .ok(
            "edit",
            json!({
                "path": "lib.rs",
                "edits": [
                    {"oldText": "fn a() {}\n", "newText": "fn a2() {}\n"},
                    {"oldText": "let s = \"hi\";", "newText": "let s = \"bye\";"}
                ]
            }),
        )
        .await;
    assert_eq!(edited, "Successfully replaced 2 block(s) in lib.rs.");
    assert_eq!(
        std::fs::read_to_string(demo.join("lib.rs")).unwrap(),
        "fn a2() {}\nlet s = \"bye\";\nfn b() {}\n"
    );

    let denied = client
        .err(
            "edit",
            json!({
                "path": outside.join("other.rs").to_string_lossy(),
                "edits": [{"oldText": "fn a", "newText": "fn z"}]
            }),
        )
        .await;
    assert!(denied.starts_with("Permission denied"), "{denied}");
    assert_eq!(
        std::fs::read_to_string(outside.join("other.rs")).unwrap(),
        "fn a() {}\n"
    );

    runner.stop().await;
}
