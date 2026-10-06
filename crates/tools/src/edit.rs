//! The `edit` tool, following pi's `edit.ts`.

use std::io::ErrorKind;

use serde::Deserialize;
use serde_json::{Map, Value};
use telehand_proto::ToolOutput;

use crate::{
    Project,
    edit_diff::{
        Edit, apply_edits_to_normalized_content, detect_line_ending, normalize_to_lf,
        restore_line_endings,
    },
    path, queue, scope,
};

#[derive(Deserialize)]
struct RawEdit {
    #[serde(rename = "oldText")]
    old_text: String,
    #[serde(rename = "newText")]
    new_text: String,
}

#[derive(Deserialize)]
struct Args {
    path: String,
    #[serde(default)]
    edits: Option<Value>,
}

fn is_single_edit(value: &Value) -> bool {
    value.get("oldText").is_some_and(Value::is_string)
        && value.get("newText").is_some_and(Value::is_string)
        && value.is_object()
}

/// Accept the argument shapes models send in practice (pi's `prepareEditArguments`):
/// `edits` as a JSON string, a single edit object instead of an array, and
/// legacy top-level `oldText`/`newText`.
fn prepare_arguments(mut args: Value) -> Value {
    let Some(obj) = args.as_object_mut() else {
        return args;
    };
    match obj.get("edits") {
        Some(Value::String(text)) => {
            if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                if parsed.is_array() {
                    obj.insert("edits".into(), parsed);
                } else if is_single_edit(&parsed) {
                    obj.insert("edits".into(), Value::Array(vec![parsed]));
                }
            }
        }
        Some(single) if is_single_edit(single) => {
            let single = single.clone();
            obj.insert("edits".into(), Value::Array(vec![single]));
        }
        _ => {}
    }

    if let (Some(Value::String(old)), Some(Value::String(new))) =
        (obj.get("oldText"), obj.get("newText"))
    {
        let mut legacy = Map::new();
        legacy.insert("oldText".into(), Value::String(old.clone()));
        legacy.insert("newText".into(), Value::String(new.clone()));
        let mut edits = match obj.get("edits") {
            Some(Value::Array(edits)) => edits.clone(),
            _ => Vec::new(),
        };
        edits.push(Value::Object(legacy));
        obj.remove("oldText");
        obj.remove("newText");
        obj.insert("edits".into(), Value::Array(edits));
    }
    args
}

fn parse(args: Value) -> Result<(String, Vec<Edit>), String> {
    let args: Args = serde_json::from_value(prepare_arguments(args))
        .map_err(|e| format!("Invalid arguments for edit: {e}"))?;
    let edits = match args.edits {
        Some(Value::Array(edits)) if !edits.is_empty() => edits,
        _ => {
            return Err(
                "Edit tool input is invalid. edits must contain at least one replacement."
                    .to_string(),
            );
        }
    };
    let edits = edits
        .into_iter()
        .map(|edit| {
            serde_json::from_value::<RawEdit>(edit).map(|edit| Edit {
                old_text: edit.old_text,
                new_text: edit.new_text,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Invalid arguments for edit: {e}"))?;
    Ok((args.path, edits))
}

fn error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::NotFound => "ENOENT",
        ErrorKind::PermissionDenied => "EACCES",
        ErrorKind::IsADirectory => "EISDIR",
        ErrorKind::NotADirectory => "ENOTDIR",
        _ => "EIO",
    }
}

pub async fn run(project: &Project, args: Value) -> ToolOutput {
    let (path, edits) = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => return ToolOutput::error(message),
    };
    let target = path::resolve(&project.main_folder, &path);
    let real = match scope::check_writable(project, &target) {
        Ok(real) => real,
        Err(message) => return ToolOutput::error(message),
    };
    let _guard = queue::lock(&real).await;

    match tokio::fs::metadata(&target).await {
        Err(e) => {
            return ToolOutput::error(format!(
                "Could not edit file: {path}. Error code: {}.",
                error_code(e.kind())
            ));
        }
        Ok(meta) if meta.permissions().readonly() => {
            return ToolOutput::error(format!(
                "Could not edit file: {path}. Error code: EACCES."
            ));
        }
        Ok(meta) if meta.is_dir() => {
            return ToolOutput::error("EISDIR: illegal operation on a directory, read");
        }
        Ok(_) => {}
    }
    let bytes = match tokio::fs::read(&target).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return ToolOutput::error(format!(
                "Could not edit file: {path}. Error code: {}.",
                error_code(e.kind())
            ));
        }
    };
    let raw = String::from_utf8_lossy(&bytes);

    // The model will not include an invisible BOM in oldText.
    let (bom, content) = match raw.strip_prefix('\u{FEFF}') {
        Some(rest) => ("\u{FEFF}", rest),
        None => ("", raw.as_ref()),
    };
    let original_ending = detect_line_ending(content);
    let normalized = normalize_to_lf(content);
    let new_content = match apply_edits_to_normalized_content(&normalized, &edits, &path) {
        Ok(new_content) => new_content,
        Err(message) => return ToolOutput::error(message),
    };
    let final_content = format!(
        "{bom}{}",
        restore_line_endings(&new_content, original_ending)
    );
    if let Err(e) = tokio::fs::write(&target, final_content).await {
        return ToolOutput::error(format!("{e}, open '{}'", target.display()));
    }
    ToolOutput::text(format!(
        "Successfully replaced {} block(s) in {path}.",
        edits.len()
    ))
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

    #[test]
    fn lenient_argument_shapes_are_accepted() {
        let edit = json!({"oldText": "a", "newText": "b"});
        let expected = vec![Edit {
            old_text: "a".into(),
            new_text: "b".into(),
        }];
        for args in [
            json!({"path": "f", "edits": [edit.clone()]}),
            json!({"path": "f", "edits": edit.clone()}),
            json!({"path": "f", "edits": edit.to_string()}),
            json!({"path": "f", "edits": json!([edit.clone()]).to_string()}),
            json!({"path": "f", "oldText": "a", "newText": "b"}),
        ] {
            assert_eq!(parse(args.clone()).unwrap().1, expected, "{args}");
        }
        let both = parse(json!({
            "path": "f",
            "edits": [{"oldText": "x", "newText": "y"}],
            "oldText": "a",
            "newText": "b"
        }))
        .unwrap()
        .1;
        assert_eq!(both.len(), 2);
        assert_eq!(both[1], expected[0]);
    }

    #[test]
    fn empty_edits_are_invalid() {
        for args in [json!({"path": "f"}), json!({"path": "f", "edits": []})] {
            assert_eq!(
                parse(args).unwrap_err(),
                "Edit tool input is invalid. edits must contain at least one replacement."
            );
        }
    }

    #[tokio::test]
    async fn preserves_crlf_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("win.txt");
        std::fs::write(&file, "\u{FEFF}first\r\nsecond\r\nthird\r\n").unwrap();
        let out = run(
            &project(dir.path()),
            json!({"path": "win.txt", "edits": [{"oldText": "second\nthird", "newText": "2\n3"}]}),
        )
        .await;
        assert!(!out.is_error, "{}", text(&out));
        assert_eq!(text(&out), "Successfully replaced 1 block(s) in win.txt.");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "\u{FEFF}first\r\n2\r\n3\r\n"
        );
    }

    #[tokio::test]
    async fn missing_file_and_outside_scope_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(tmp.path().join("outside.txt"), "abc").unwrap();
        let p = project(&main);

        let out = run(
            &p,
            json!({"path": "nope.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
        )
        .await;
        assert_eq!(text(&out), "Could not edit file: nope.txt. Error code: ENOENT.");

        let out = run(
            &p,
            json!({"path": "../outside.txt", "edits": [{"oldText": "a", "newText": "b"}]}),
        )
        .await;
        assert!(text(&out).starts_with("Permission denied"), "{}", text(&out));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("outside.txt")).unwrap(),
            "abc"
        );
    }
}
