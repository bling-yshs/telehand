use telehand_e2e::{Env, json};

#[tokio::test]
async fn large_files_are_paged_through_mcp() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let content: String = (1..=2500).map(|i| format!("line {i}\n")).collect();
    std::fs::write(demo.join("big.txt"), content).unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;

    let first = client.ok("read", json!({"path": "big.txt"})).await;
    assert!(
        first.ends_with("[Showing lines 1-2000 of 2501. Use offset=2001 to continue.]"),
        "{}",
        &first[first.len() - 200..]
    );

    let rest = client
        .ok("read", json!({"path": "big.txt", "offset": 2001}))
        .await;
    assert!(rest.starts_with("line 2001\n"), "{rest}");
    assert!(rest.ends_with("line 2500\n"), "{rest}");

    let beyond = client
        .err("read", json!({"path": "big.txt", "offset": 9999}))
        .await;
    assert_eq!(beyond, "Offset 9999 is beyond end of file (2501 lines total)");

    runner.stop().await;
}
