#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use telehand_e2e::{Client, Env, json, mcp_status};
#[cfg(unix)]
use telehand_runner::{RunExit, RunnerConfig};
use telehand_server::{admin, keys};

// The admin socket that reaches a running server exists on Unix only.
#[cfg(unix)]
#[tokio::test]
async fn keys_created_while_running_work_and_survive_persistence() {
    let env = Env::start().await;
    let demo = env.dir("demo");

    let new_key = admin::create_key(&env.data_dir, Some("laptop".into()))
        .await
        .unwrap();
    let listed = admin::list_keys(&env.data_dir).await.unwrap();
    assert_eq!(listed[&new_key].name.as_deref(), Some("laptop"));
    assert!(listed.contains_key(&env.key));

    // The new key works immediately for a runner and an agent.
    let config = RunnerConfig {
        key: new_key.clone(),
        ..env.runner_config(&[("demo", &demo)])
    };
    let runner = env.start_runner(config);
    let client = Client::connect(&telehand_proto::mcp_url(&env.server_url, &new_key)).await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    // Once the server writes keys.json, the new key is in it (not overwritten).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let stored = keys::load(&env.data_dir).unwrap();
    assert_eq!(stored[&new_key].current_project.as_deref(), Some("demo"));
    assert!(stored.contains_key(&env.key));

    runner.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn removing_a_key_disconnects_its_runner_and_endpoint() {
    let env = Env::start().await;
    let demo = env.dir("demo");

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    admin::remove_key(&env.data_dir, &env.key).await.unwrap();
    assert_eq!(runner.exit().await, RunExit::KeyRejected);
    assert_eq!(mcp_status(&env.mcp_url()).await, 404);

    // A runner using the removed key is rejected the same way.
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    assert_eq!(runner.exit().await, RunExit::KeyRejected);

    assert!(admin::remove_key(&env.data_dir, &env.key).await.is_err());
}

#[tokio::test]
async fn key_commands_edit_the_file_when_no_server_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    let key = admin::create_key(&data_dir, None).await.unwrap();
    assert!(
        admin::list_keys(&data_dir)
            .await
            .unwrap()
            .contains_key(&key)
    );
    admin::remove_key(&data_dir, &key).await.unwrap();
    assert!(keys::load(&data_dir).unwrap().is_empty());
    assert!(admin::remove_key(&data_dir, &key).await.is_err());
}
