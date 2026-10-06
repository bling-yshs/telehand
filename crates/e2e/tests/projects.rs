use telehand_e2e::{Env, json};

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
