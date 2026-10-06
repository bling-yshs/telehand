//! The `write` tool, following pi's `write.ts`.

use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;

use crate::{Project, path, queue, scope};

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

pub async fn run(project: &Project, args: Value) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for write: {e}")),
    };
    let target = path::resolve(&project.main_folder, &args.path);
    let real = match scope::check_writable(project, &target) {
        Ok(real) => real,
        Err(message) => return ToolOutput::error(message),
    };
    let _guard = queue::lock(&real).await;
    if let Some(dir) = target.parent()
        && let Err(e) = tokio::fs::create_dir_all(dir).await
    {
        return ToolOutput::error(format!("{e}, mkdir '{}'", dir.display()));
    }
    if let Err(e) = tokio::fs::write(&target, args.content.as_bytes()).await {
        return ToolOutput::error(format!("{e}, open '{}'", target.display()));
    }
    ToolOutput::text(format!("Successfully wrote to {}", args.path))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;
    use telehand_proto::Content;

    use super::*;

    fn project(dir: &Path) -> Project {
        Project {
            main_folder: dir.to_path_buf(),
            extra_folders: Vec::new(),
        }
    }

    fn text(output: &ToolOutput) -> &str {
        match &output.content[0] {
            Content::Text { text } => text,
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn creates_parents_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path());
        let out = run(&p, json!({"path": "a/b/c.txt", "content": "one"})).await;
        assert!(!out.is_error, "{}", text(&out));
        assert_eq!(text(&out), "Successfully wrote to a/b/c.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "one"
        );

        run(&p, json!({"path": "a/b/c.txt", "content": "two"})).await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b/c.txt")).unwrap(),
            "two"
        );
    }

    #[tokio::test]
    async fn refuses_to_write_outside_the_project() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        let out = run(
            &project(&main),
            json!({"path": "../escape.txt", "content": "x"}),
        )
        .await;
        assert!(out.is_error);
        assert!(text(&out).starts_with("Permission denied"), "{}", text(&out));
        assert!(!tmp.path().join("escape.txt").exists());
    }

    #[tokio::test]
    async fn concurrent_writes_to_one_file_do_not_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let p = project(dir.path());
        let big = "x".repeat(1 << 20);
        let writes = (0..8).map(|i| {
            let p = p.clone();
            let content = format!("{i}{big}");
            tokio::spawn(async move {
                run(&p, json!({"path": "same.txt", "content": content})).await
            })
        });
        for write in writes {
            assert!(!write.await.unwrap().is_error);
        }
        let result = std::fs::read_to_string(dir.path().join("same.txt")).unwrap();
        assert_eq!(result.len(), big.len() + 1);
    }
}
