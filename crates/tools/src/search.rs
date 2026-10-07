//! Running the external search programs behind `grep` (ripgrep) and `find`
//! (fd). Both are expected to be on PATH.

use std::process::Stdio;

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

pub enum Finished {
    /// The program exited by itself.
    Exited { code: Option<i32>, stderr: String },
    /// `on_line` asked to stop; the program was killed.
    Stopped,
    /// `cancel` fired; the program was killed.
    Aborted,
}

/// Run `cmd`, passing each line of its stdout to `on_line` until that returns false.
pub async fn stream_lines(
    mut cmd: Command,
    cancel: &CancellationToken,
    mut on_line: impl FnMut(&str) -> bool,
) -> std::io::Result<Finished> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let stderr = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    let mut stopped = false;
    loop {
        tokio::select! {
            read = reader.read_until(b'\n', &mut line) => {
                if !matches!(read, Ok(n) if n > 0) {
                    break;
                }
                let text = String::from_utf8_lossy(&line);
                let text = text.strip_suffix('\n').unwrap_or(&text);
                if !on_line(text) {
                    stopped = true;
                    break;
                }
                line.clear();
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                stderr.abort();
                return Ok(Finished::Aborted);
            }
        }
    }
    if stopped {
        let _ = child.kill().await;
        stderr.abort();
        return Ok(Finished::Stopped);
    }
    let status = child.wait().await?;
    Ok(Finished::Exited {
        code: status.code(),
        stderr: stderr.await.unwrap_or_default(),
    })
}

/// How JavaScript prints a missing exit code, as in pi's messages.
pub fn code_text(code: Option<i32>) -> String {
    code.map_or_else(|| "null".to_string(), |code| code.to_string())
}
