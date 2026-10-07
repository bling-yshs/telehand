//! grep and find need ripgrep (`rg`) and fd (`fd`) on PATH.

use std::path::Path;

use telehand_e2e::{Client, Env, Runner, json};

async fn project_with_files(env: &Env) -> (Runner, Client) {
    let demo = env.dir("demo");
    write(&demo, "main.rs", "fn main() {\n    helper();\n}\n");
    write(&demo, "src/lib.rs", "pub fn helper() {}\n");
    write(&demo, "notes.txt", "helper notes\n");
    let runner = env.start_runner(env.runner_config(&[("demo", &demo)]));
    let client = env.client().await;
    client.wait_online().await;
    client.ok("select_project", json!({"name": "demo"})).await;
    (runner, client)
}

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// Output lines in a stable order (rg and fd search in parallel).
fn sorted(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort();
    lines
}

#[tokio::test]
async fn grep_finds_matching_lines() {
    let env = Env::start().await;
    let (runner, client) = project_with_files(&env).await;

    let out = client.ok("grep", json!({"pattern": "helper"})).await;
    assert_eq!(
        sorted(&out),
        [
            "main.rs:2:     helper();",
            "notes.txt:1: helper notes",
            "src/lib.rs:1: pub fn helper() {}",
        ]
    );

    let out = client
        .ok(
            "grep",
            json!({"pattern": "helper", "path": "main.rs", "context": 1}),
        )
        .await;
    assert_eq!(out, "main.rs-1- fn main() {\nmain.rs:2:     helper();\nmain.rs-3- }");

    let out = client
        .ok("grep", json!({"pattern": "HELPER", "ignoreCase": true, "glob": "*.rs"}))
        .await;
    assert_eq!(sorted(&out).len(), 2, "{out}");

    let out = client
        .ok("grep", json!({"pattern": "helper", "limit": 1}))
        .await;
    assert!(
        out.ends_with("\n\n[1 matches limit reached. Use limit=2 for more, or refine pattern]"),
        "{out}"
    );

    let out = client.ok("grep", json!({"pattern": "nothing here"})).await;
    assert_eq!(out, "No matches found");

    let error = client
        .err("grep", json!({"pattern": "x", "path": "missing"}))
        .await;
    assert!(error.starts_with("Path not found: "), "{error}");

    runner.stop().await;
}

#[tokio::test]
async fn find_matches_file_names_and_paths() {
    let env = Env::start().await;
    let (runner, client) = project_with_files(&env).await;

    let out = client.ok("find", json!({"pattern": "*.rs"})).await;
    assert_eq!(sorted(&out), ["main.rs", "src/lib.rs"]);

    let out = client.ok("find", json!({"pattern": "src/*.rs"})).await;
    assert_eq!(out, "src/lib.rs");

    let out = client.ok("find", json!({"pattern": "*.rs", "path": "src"})).await;
    assert_eq!(out, "lib.rs");

    let out = client.ok("find", json!({"pattern": "*.md"})).await;
    assert_eq!(out, "No files found matching pattern");

    runner.stop().await;
}
