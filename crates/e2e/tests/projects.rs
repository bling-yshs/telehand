use std::time::Duration;

use telehand_e2e::{Client, Env, json};
use telehand_runner::RunnerConfig;

#[tokio::test]
async fn extra_folders_extend_the_write_scope_after_restart() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let shared = env.dir("shared");
    let target = shared.join("config.toml");
    let args = json!({"path": target.to_string_lossy(), "content": "x = 1\n"});

    // Without the extra folder, writing there is denied.
    let mut config = env.runner_config(&[("demo", &demo)]);
    let runner = env.start_runner(config.clone());
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;
    let denied = client.err("write", args.clone()).await;
    assert!(denied.starts_with("Permission denied"), "{denied}");
    runner.stop().await;
    client.wait_offline().await;

    // After adding it and restarting the runner, the write succeeds.
    config.add_folder("demo", &shared).unwrap();
    let runner = env.start_runner(config);
    client.wait_online().await;
    let current = client.ok("current_project", json!({})).await;
    assert!(current.contains(&*shared.to_string_lossy()), "{current}");
    let list = client.ok("list_project", json!({})).await;
    assert!(list.contains("extra folders"), "{list}");

    client.ok("write", args).await;
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "x = 1\n");

    // Relative paths still resolve against the main folder.
    client
        .ok("write", json!({"path": "rel.txt", "content": "r"}))
        .await;
    assert!(demo.join("rel.txt").exists());

    runner.stop().await;
}

#[tokio::test]
async fn project_changes_take_effect_without_restarting_the_runner() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let other = env.dir("other");
    let path = env.tmp.path().join("config").join("runner.json");
    let runner = env.start_runner_watching(&path, env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    // Added the way `telehand-runner project add` does: load, change, save.
    let mut config = RunnerConfig::load(&path).unwrap();
    config.add_project(&other, Some("other")).unwrap();
    config.save(&path).unwrap();
    wait_for_list(&client, |list| list.contains("other")).await;
    client.ok("select_project", json!({"name": "other"})).await;
    client
        .ok("write", json!({"path": "new.txt", "content": "x"}))
        .await;
    assert!(other.join("new.txt").exists());

    // Removed: the selection no longer resolves.
    let mut config = RunnerConfig::load(&path).unwrap();
    config.remove_project("other").unwrap();
    config.save(&path).unwrap();
    wait_for_list(&client, |list| !list.contains("other")).await;
    let err = client.err("read", json!({"path": "new.txt"})).await;
    assert!(err.contains("no longer exists"), "{err}");

    // A config that fails to load keeps the previous projects.
    std::fs::write(&path, "{ not json").unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let list = client.ok("list_project", json!({})).await;
    assert!(list.contains("demo"), "{list}");

    runner.stop().await;
}

/// Poll `list_project` until its output satisfies `done`.
async fn wait_for_list(client: &Client, done: impl Fn(&str) -> bool) {
    for _ in 0..100 {
        let list = client.ok("list_project", json!({})).await;
        if done(&list) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the project list did not change");
}
