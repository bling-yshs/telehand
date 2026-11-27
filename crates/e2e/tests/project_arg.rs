use telehand_e2e::{Env, json};

#[tokio::test]
async fn agents_on_one_url_work_in_different_projects() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let other = env.dir("other");
    std::fs::write(demo.join("a.txt"), "from demo").unwrap();
    std::fs::write(other.join("a.txt"), "from other").unwrap();

    let runner = env.start_runner(env.runner_config(&[("demo", &demo), ("other", &other)]));
    let first = env.client().await;
    let second = env.client().await;
    first.wait_online().await;

    // Interleaved calls: neither agent's project affects the other's.
    let read = |project: &str| json!({"project": project, "path": "a.txt"});
    assert_eq!(first.ok("read", read("demo")).await, "from demo");
    assert_eq!(second.ok("read", read("other")).await, "from other");
    assert_eq!(first.ok("read", read("demo")).await, "from demo");
    // One agent can switch projects from one call to the next.
    assert_eq!(first.ok("read", read("other")).await, "from other");

    // Relative paths and bash resolve against the named project.
    second
        .ok(
            "write",
            json!({"project": "other", "path": "b.txt", "content": "b"}),
        )
        .await;
    assert!(other.join("b.txt").exists());
    let cwd = first
        .ok("bash", json!({"project": "demo", "command": "ls"}))
        .await;
    assert!(cwd.contains("a.txt") && !cwd.contains("b.txt"), "{cwd}");
    runner.stop().await;
}

#[tokio::test]
async fn file_tools_require_a_project_on_the_runner() {
    let env = Env::start().await;
    let demo = env.dir("demo");
    let other = env.dir("other");

    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;

    for (tool, args) in [
        ("read", json!({"path": "a.txt"})),
        ("write", json!({"path": "a.txt", "content": "x"})),
        (
            "edit",
            json!({"path": "a.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
        ),
        ("bash", json!({"command": "echo hi"})),
        ("grep", json!({"pattern": "x"})),
        ("find", json!({"pattern": "*.rs"})),
    ] {
        let error = client.err(tool, args).await;
        assert!(
            error.contains("Missing required argument: project"),
            "{tool}: {error}"
        );
        assert!(error.contains("Available projects: demo"), "{tool}: {error}");
    }
    runner.stop().await;
    client.wait_offline().await;

    // The runner restarts without the project.
    let runner = env.start_runner(env.runner_config(&[("other", &other)]));
    client.wait_online().await;
    let error = client
        .err(
            "write",
            json!({"project": "demo", "path": "a.txt", "content": "x"}),
        )
        .await;
    assert!(error.contains("Project 'demo' not found"), "{error}");
    assert!(error.contains("Available projects: other"), "{error}");
    runner.stop().await;
}

#[tokio::test]
async fn tools_that_work_in_a_project_require_the_argument() {
    let env = Env::start().await;
    let client = env.client().await;
    let tools = client.tools().await;
    for tool in tools {
        let schema = serde_json::Value::Object((*tool.input_schema).clone());
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        let takes_project = required.contains(&json!("project"));
        let expected = !["list_project", "bash_result", "bash_kill"].contains(&&*tool.name);
        assert_eq!(takes_project, expected, "{}: {schema}", tool.name);
        if expected {
            assert!(schema["properties"]["project"].is_object(), "{schema}");
        }
    }
}
