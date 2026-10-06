use telehand_e2e::{Env, json};

#[tokio::test]
async fn current_project_survives_a_server_restart() {
    let mut env = Env::start().await;
    let demo = env.dir("demo");
    std::fs::write(demo.join("a.txt"), "hello").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;
    runner.stop().await;

    env.restart_server().await;
    let keys = telehand_server::keys::load(&env.data_dir).unwrap();
    assert_eq!(keys[&env.key].current_project.as_deref(), Some("demo"));

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    let current = client.ok("current_project", json!({})).await;
    assert!(current.contains("demo"), "{current}");
    assert_eq!(client.ok("read", json!({"path": "a.txt"})).await, "hello");
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
