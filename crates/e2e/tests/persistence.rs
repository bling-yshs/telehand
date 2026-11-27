use telehand_e2e::{Env, json};

#[tokio::test]
async fn sessions_on_one_url_select_projects_independently() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let other = env.dir("other");
    std::fs::write(demo.join("a.txt"), "from demo").unwrap();
    std::fs::write(other.join("a.txt"), "from other").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo), ("other", &other)]));
    let first = env.client().await;
    let second = env.client().await;
    first.wait_online().await;

    first.ok("select_project", json!({"name": "demo"})).await;
    // The second session has not selected anything yet.
    let error = second.err("read", json!({"path": "a.txt"})).await;
    assert!(error.contains("No project selected"), "{error}");

    second.ok("select_project", json!({"name": "other"})).await;
    assert_eq!(first.ok("read", json!({"path": "a.txt"})).await, "from demo");
    assert_eq!(second.ok("read", json!({"path": "a.txt"})).await, "from other");

    // Each entry is "- <name>\n  main folder: ...", marked " (current)" at its end.
    let list = first.ok("list_project", json!({})).await;
    let current: Vec<&str> = list
        .split("\n- ")
        .filter(|entry| entry.ends_with("(current)"))
        .collect();
    assert_eq!(current.len(), 1, "{list}");
    assert!(current[0].trim_start_matches("- ").starts_with("demo"), "{list}");
    let current = second.ok("current_project", json!({})).await;
    assert!(current.contains("other"), "{current}");
    runner.stop().await;
}

#[tokio::test]
async fn the_selection_does_not_survive_a_server_restart() {
    let mut env = Env::start().await;
    let demo = env.dir("demo");

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;
    runner.stop().await;

    env.restart_server().await;
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    let error = client.err("read", json!({"path": "a.txt"})).await;
    assert!(error.contains("No project selected"), "{error}");
    let current = client.ok("current_project", json!({})).await;
    assert!(current.contains("No project selected"), "{current}");
    runner.stop().await;
}

#[tokio::test]
async fn file_tools_require_an_existing_selected_project() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let other = env.dir("other");

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    let error = client.err("read", json!({"path": "a.txt"})).await;
    assert!(error.contains("No project selected"), "{error}");
    assert!(error.contains("select_project"), "{error}");

    client.ok("select_project", json!({"name": "demo"})).await;
    runner.stop().await;
    client.wait_offline().await;

    // The runner restarts without the selected project.
    let runner = env.start_runner(env.runner_config(&[("other", &other)]));
    client.wait_online().await;
    let error = client
        .err("write", json!({"path": "a.txt", "content": "x"}))
        .await;
    assert!(error.contains("no longer exists"), "{error}");
    let error = client.err("current_project", json!({})).await;
    assert!(error.contains("no longer exists"), "{error}");
    runner.stop().await;
}
