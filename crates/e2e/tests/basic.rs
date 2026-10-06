use telehand_e2e::{Env, json, mcp_status};

#[tokio::test]
async fn agent_selects_a_project_and_reads_files() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    std::fs::create_dir_all(demo.join("src")).unwrap();
    std::fs::write(demo.join("src/main.rs"), "fn main() {}\n").unwrap();
    let outside = env.dir("outside");
    std::fs::write(outside.join("notes.txt"), "outside notes").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    let tools = client.tool_names().await;
    for name in ["list_project", "select_project", "current_project", "read"] {
        assert!(
            tools.iter().any(|t| t == name),
            "missing tool {name}: {tools:?}"
        );
    }

    let list = client.ok("list_project", json!({})).await;
    assert!(list.contains("demo"), "{list}");
    assert!(list.contains(&*demo.to_string_lossy()), "{list}");

    let none = client.ok("current_project", json!({})).await;
    assert!(none.contains("No project selected"), "{none}");

    let missing = client.err("select_project", json!({"name": "nope"})).await;
    assert!(missing.contains("not found"), "{missing}");

    client.ok("select_project", json!({"name": "demo"})).await;
    let current = client.ok("current_project", json!({})).await;
    assert!(current.contains("demo"), "{current}");
    let list = client.ok("list_project", json!({})).await;
    assert!(list.contains("(current)"), "{list}");

    let relative = client.ok("read", json!({"path": "src/main.rs"})).await;
    assert_eq!(relative, "fn main() {}\n");

    let absolute_path = outside.join("notes.txt");
    let absolute = client
        .ok("read", json!({"path": absolute_path.to_string_lossy()}))
        .await;
    assert_eq!(absolute, "outside notes");

    runner.stop().await;
}

#[tokio::test]
async fn unknown_key_is_not_found() {
    let env = Env::start().await;
    let url = format!(
        "{}/mcp/00000000-0000-0000-0000-000000000000",
        env.server_url
    );
    assert_eq!(mcp_status(&url).await, 404);
}
