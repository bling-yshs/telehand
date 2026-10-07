use std::time::{Duration, Instant};

use telehand_e2e::{Env, json};
use telehand_runner::RunExit;

#[tokio::test]
async fn every_tool_fails_fast_after_the_runner_disconnects() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;
    runner.stop().await;
    client.wait_offline().await;

    let tools = client.tool_names().await;
    assert_eq!(tools.len(), 11, "{tools:?}");

    for (tool, args) in [
        ("list_project", json!({})),
        ("select_project", json!({"name": "demo"})),
        ("current_project", json!({})),
        ("read", json!({"path": "a.txt"})),
        ("write", json!({"path": "a.txt", "content": "x"})),
        (
            "edit",
            json!({"path": "a.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
        ),
        ("bash", json!({"command": "echo hi"})),
        ("bash_result", json!({"task_id": "bash-1"})),
        ("bash_kill", json!({"task_id": "bash-1"})),
        ("grep", json!({"pattern": "x"})),
        ("find", json!({"pattern": "*.rs"})),
    ] {
        let started = Instant::now();
        let error = client.err(tool, args).await;
        assert!(error.contains("Runner offline"), "{tool}: {error}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{tool} was slow"
        );
    }
}

#[tokio::test]
async fn a_new_runner_replaces_the_old_one() {
    let env = Env::start().await;
    let first_dir = env.dir("first");
    let second_dir = env.dir("second");

    let first = env.start_runner(env.runner_config(&[("first", &first_dir)]));
    let client = env.client().await;
    client.wait_online().await;

    let second = env.start_runner(env.runner_config(&[("second", &second_dir)]));
    assert_eq!(first.exit().await, RunExit::Replaced);

    // The server now serves the second runner's projects.
    let mut list = String::new();
    for _ in 0..50 {
        list = client.ok("list_project", json!({})).await;
        if list.contains("second") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(list.contains("second") && !list.contains("first"), "{list}");
    second.stop().await;
}

#[tokio::test]
async fn the_runner_reconnects_after_the_server_restarts() {
    let mut env = Env::start().await;
    let demo = env.dir("demo");
    std::fs::write(demo.join("a.txt"), "still here").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    env.restart_server_same_port().await;
    let client = env.client().await;
    client.wait_online().await;
    assert_eq!(
        client.ok("read", json!({"path": "a.txt"})).await,
        "still here"
    );
    runner.stop().await;
}

#[tokio::test]
async fn register_checks_the_key_without_kicking_the_runner() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    telehand_runner::check_key(&env.server_url, &env.key)
        .await
        .unwrap();
    let error = telehand_runner::check_key(&env.server_url, "00000000-0000-0000-0000-000000000000")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("rejected"), "{error:#}");
    assert!(
        telehand_runner::check_key("http://127.0.0.1:1", &env.key)
            .await
            .is_err()
    );

    // The running runner is still connected.
    client.ok("list_project", json!({})).await;
    runner.stop().await;
}
