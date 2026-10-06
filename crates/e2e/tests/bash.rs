use std::time::{Duration, Instant};

use telehand_e2e::{Env, json};

#[tokio::test]
async fn bash_runs_in_the_current_project() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    std::fs::write(demo.join("a.txt"), "hello").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    let error = client.err("bash", json!({"command": "pwd"})).await;
    assert!(error.contains("No project selected"), "{error}");

    client.ok("select_project", json!({"name": "demo"})).await;
    let out = client
        .ok("bash", json!({"command": "pwd && cat a.txt"}))
        .await;
    assert_eq!(out, format!("{}\nhello", demo.display()));

    let error = client
        .err("bash", json!({"command": "echo failing >&2; exit 2"}))
        .await;
    assert_eq!(error, "failing\n\n\nCommand exited with code 2");

    runner.stop().await;
}

#[tokio::test]
async fn command_timeouts_kill_the_command_end_to_end() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    let started = Instant::now();
    let error = client
        .err("bash", json!({"command": "sleep 30", "timeout": 1}))
        .await;
    assert_eq!(error, "Command timed out after 1 seconds");
    assert!(started.elapsed() < Duration::from_secs(10));

    runner.stop().await;
}
