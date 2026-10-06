//! The `bash` tool, following pi's `bash.ts`: runs a command in the project's
//! main folder and returns stdout and stderr combined. Only the last 2000 lines
//! / 50KB are returned; when output is cut, the full output is saved to a temp
//! file whose path is shown.

use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::mpsc,
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Project,
    shell::{self, ShellConfig},
    truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, TruncatedBy, format_size, truncate_tail},
};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483.647;
/// After the shell exits, keep reading until its pipes stay quiet this long:
/// background processes it started may hold them open.
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);
/// How much of the output's end is kept in memory for the result.
const ROLLING_BYTES: usize = DEFAULT_MAX_BYTES * 2;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout: Option<f64>,
}

pub async fn run(project: &Project, args: Value, cancel: &CancellationToken) -> ToolOutput {
    let args: Args = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for bash: {e}")),
    };
    let timeout = match args.timeout.map(resolve_timeout).transpose() {
        Ok(timeout) => timeout,
        Err(message) => return ToolOutput::error(message),
    };
    if cancel.is_cancelled() {
        return ToolOutput::error("Command aborted");
    }
    let shell = match shell::shell_config() {
        Ok(shell) => shell,
        Err(message) => return ToolOutput::error(message),
    };
    let cwd = &project.main_folder;
    if !cwd.exists() {
        return ToolOutput::error(format!(
            "Working directory does not exist: {}\nCannot execute bash commands.",
            cwd.display()
        ));
    }

    let (mut output, ending) = match execute(&shell, &args.command, cwd, timeout, cancel).await {
        Ok(finished) => finished,
        Err(message) => return ToolOutput::error(message),
    };
    match ending {
        Ending::Aborted => ToolOutput::error(append_status(&output.format(""), "Command aborted")),
        Ending::TimedOut => ToolOutput::error(append_status(
            &output.format(""),
            &format!(
                "Command timed out after {} seconds",
                args.timeout.unwrap_or_default()
            ),
        )),
        Ending::Exited(0) => ToolOutput::text(output.format("(no output)")),
        Ending::Exited(code) => ToolOutput::error(append_status(
            &output.format("(no output)"),
            &format!("Command exited with code {code}"),
        )),
    }
}

fn resolve_timeout(seconds: f64) -> Result<Duration, String> {
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Invalid timeout: must be a finite number of seconds".to_string());
    }
    if seconds > MAX_TIMEOUT_SECONDS {
        return Err(format!(
            "Invalid timeout: maximum is {MAX_TIMEOUT_SECONDS} seconds"
        ));
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_string()
    } else {
        format!("{text}\n\n{status}")
    }
}

enum Ending {
    Exited(i32),
    TimedOut,
    Aborted,
}

async fn execute(
    shell: &ShellConfig,
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
    cancel: &CancellationToken,
) -> Result<(Output, Ending), String> {
    let mut cmd = Command::new(&shell.shell);
    cmd.args(&shell.args);
    if !shell.command_via_stdin {
        cmd.arg(command);
    }
    cmd.current_dir(cwd)
        .stdin(if shell.command_via_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Its own process group, so the whole tree can be killed.
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start {}: {e}", shell.shell.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(command.as_bytes()).await;
    }
    let pid = child.id();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let readers = [
        spawn_reader(child.stdout.take(), tx.clone()),
        spawn_reader(child.stderr.take(), tx),
    ];
    let mut output = Output::new();

    let far_future = Instant::now() + Duration::from_secs(86400 * 365 * 30);
    let deadline = tokio::time::sleep_until(timeout.map_or(far_future, |t| Instant::now() + t));
    tokio::pin!(deadline);
    let mut timed_out = false;
    let mut aborted = false;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            Some(chunk) = rx.recv() => output.append(&chunk),
            _ = &mut deadline, if timeout.is_some() && !timed_out => {
                timed_out = true;
                kill_tree(pid);
            }
            _ = cancel.cancelled(), if !aborted => {
                aborted = true;
                kill_tree(pid);
            }
        }
    };
    // Read what is left, without waiting on pipes that background processes keep open.
    while let Ok(Some(chunk)) = tokio::time::timeout(EXIT_STDIO_GRACE, rx.recv()).await {
        output.append(&chunk);
    }
    for reader in readers {
        reader.abort();
    }

    let ending = if aborted {
        Ending::Aborted
    } else if timed_out {
        Ending::TimedOut
    } else {
        let status = status.map_err(|e| format!("Failed to wait for the shell: {e}"))?;
        Ending::Exited(exit_code(status))
    };
    Ok((output, ending))
}

fn spawn_reader<R>(pipe: Option<R>, tx: mpsc::UnboundedSender<Vec<u8>>) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let Some(mut pipe) = pipe else { return };
        let mut buf = vec![0u8; 8192];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// The exit code; a shell killed by a signal reports 128 + the signal number.
fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// Kill the shell and everything it started.
fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(unix)]
    {
        let pid = pid as libc::pid_t;
        // SAFETY: kill(2) only sends a signal; it touches no memory of ours.
        unsafe {
            if libc::kill(-pid, libc::SIGKILL) != 0 {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let system_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let taskkill = Path::new(&system_root).join("System32").join("taskkill.exe");
        let _ = std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Collects streamed output with bounded memory (pi's `OutputAccumulator`):
/// the end of the output stays in memory, and once the output exceeds the
/// limits all of it goes to a temp file.
struct Output {
    tail: Vec<u8>,
    tail_starts_at_line_boundary: bool,
    total_bytes: usize,
    completed_lines: usize,
    current_line_bytes: usize,
    has_open_line: bool,
    /// Output not yet in the temp file (there is none while within limits).
    buffered: Vec<u8>,
    file: Option<(PathBuf, File)>,
    file_error: Option<String>,
}

impl Output {
    fn new() -> Self {
        Self {
            tail: Vec::new(),
            tail_starts_at_line_boundary: true,
            total_bytes: 0,
            completed_lines: 0,
            current_line_bytes: 0,
            has_open_line: false,
            buffered: Vec::new(),
            file: None,
            file_error: None,
        }
    }

    fn append(&mut self, data: &[u8]) {
        self.total_bytes += data.len();
        self.tail.extend_from_slice(data);
        if self.tail.len() > ROLLING_BYTES * 2 {
            self.trim_tail();
        }
        match data.iter().rposition(|&b| b == b'\n') {
            None => {
                self.current_line_bytes += data.len();
                self.has_open_line |= !data.is_empty();
            }
            Some(last) => {
                self.completed_lines += data.iter().filter(|&&b| b == b'\n').count();
                self.current_line_bytes = data.len() - last - 1;
                self.has_open_line = self.current_line_bytes > 0;
            }
        }

        if self.file.is_none() && !self.exceeds_limits() {
            self.buffered.extend_from_slice(data);
            return;
        }
        self.persist();
        self.write_to_file(data);
    }

    fn total_lines(&self) -> usize {
        self.completed_lines + usize::from(self.has_open_line)
    }

    fn exceeds_limits(&self) -> bool {
        self.total_bytes > DEFAULT_MAX_BYTES || self.total_lines() > DEFAULT_MAX_LINES
    }

    fn trim_tail(&mut self) {
        let mut start = self.tail.len() - ROLLING_BYTES;
        while start < self.tail.len() && (self.tail[start] & 0xC0) == 0x80 {
            start += 1;
        }
        self.tail_starts_at_line_boundary = self.tail[start - 1] == b'\n';
        self.tail.drain(..start);
    }

    /// Move the output into a temp file, if not done yet.
    fn persist(&mut self) {
        if self.file.is_some() || self.file_error.is_some() {
            return;
        }
        let created = tempfile::Builder::new()
            .prefix("telehand-bash-")
            .suffix(".log")
            .tempfile()
            .and_then(|file| file.keep().map_err(|e| e.error));
        match created {
            Ok((file, path)) => {
                self.file = Some((path, file));
                let buffered = std::mem::take(&mut self.buffered);
                self.write_to_file(&buffered);
            }
            Err(e) => self.file_error = Some(e.to_string()),
        }
    }

    fn write_to_file(&mut self, data: &[u8]) {
        if let Some((_, file)) = &mut self.file
            && let Err(e) = file.write_all(data)
        {
            self.file_error = Some(e.to_string());
        }
    }

    /// The text for the agent: the end of the output, with a note on where the
    /// full output is when it was cut. `empty` stands in for no output.
    fn format(&mut self, empty: &str) -> String {
        let text = String::from_utf8_lossy(&self.tail);
        let text = if self.tail_starts_at_line_boundary {
            &text[..]
        } else {
            text.split_once('\n').map_or(&text[..], |(_, rest)| rest)
        };
        let tail = truncate_tail(text, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);
        let total_lines = self.total_lines();
        let truncated = total_lines > DEFAULT_MAX_LINES || self.total_bytes > DEFAULT_MAX_BYTES;

        let mut result = if tail.content.is_empty() {
            empty.to_string()
        } else {
            tail.content.clone()
        };
        if !truncated {
            return result;
        }
        self.persist();
        let full_output = match (&self.file, &self.file_error) {
            (Some((path, _)), None) => path.display().to_string(),
            (_, Some(error)) => format!("(could not save: {error})"),
            (None, None) => "(could not save)".to_string(),
        };
        let truncated_by = tail.truncated_by.unwrap_or(if self.total_bytes > DEFAULT_MAX_BYTES {
            TruncatedBy::Bytes
        } else {
            TruncatedBy::Lines
        });
        let end_line = total_lines;
        let start_line = (total_lines + 1).saturating_sub(tail.output_lines);
        if tail.last_line_partial {
            result.push_str(&format!(
                "\n\n[Showing last {} of line {end_line} (line is {}). Full output: {full_output}]",
                format_size(tail.output_bytes),
                format_size(self.current_line_bytes)
            ));
        } else if truncated_by == TruncatedBy::Lines {
            result.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {total_lines}. Full output: {full_output}]"
            ));
        } else {
            result.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {total_lines} ({} limit). Full output: {full_output}]",
                format_size(DEFAULT_MAX_BYTES)
            ));
        }
        result
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Instant;

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

    async fn bash(dir: &Path, args: Value) -> ToolOutput {
        run(&project(dir), args, &CancellationToken::new()).await
    }

    #[tokio::test]
    async fn runs_in_the_main_folder_and_returns_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(dir.path(), json!({"command": "pwd; echo oops >&2"})).await;
        assert!(!out.is_error, "{}", text(&out));
        let real = dir.path().canonicalize().unwrap();
        assert_eq!(text(&out), format!("{}\noops\n", real.display()));

        let out = bash(dir.path(), json!({"command": "true"})).await;
        assert_eq!(text(&out), "(no output)");
    }

    #[tokio::test]
    async fn a_failing_command_is_an_error_with_its_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(dir.path(), json!({"command": "echo out; exit 3"})).await;
        assert!(out.is_error);
        assert_eq!(text(&out), "out\n\n\nCommand exited with code 3");

        let out = bash(dir.path(), json!({"command": "exit 1"})).await;
        assert_eq!(text(&out), "(no output)\n\nCommand exited with code 1");
    }

    #[tokio::test]
    async fn timeouts_kill_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let out = bash(
            dir.path(),
            json!({"command": "echo start; sleep 10", "timeout": 0.5}),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(out.is_error);
        assert_eq!(
            text(&out),
            "start\n\n\nCommand timed out after 0.5 seconds"
        );

        let out = bash(dir.path(), json!({"command": "true", "timeout": 0})).await;
        assert_eq!(
            text(&out),
            "Invalid timeout: must be a finite number of seconds"
        );
    }

    #[tokio::test]
    async fn cancelling_kills_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            token.cancel();
        });
        let started = Instant::now();
        let out = run(&project(dir.path()), json!({"command": "sleep 10"}), &cancel).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(text(&out), "Command aborted");
    }

    #[tokio::test]
    async fn background_processes_do_not_hold_up_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let out = bash(dir.path(), json!({"command": "sleep 5 & echo done"})).await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(text(&out), "done\n");
    }

    #[tokio::test]
    async fn long_output_keeps_the_end_and_saves_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(dir.path(), json!({"command": "seq 1 3000"})).await;
        assert!(!out.is_error);
        let text = text(&out);
        assert!(text.starts_with("1001\n1002\n"), "{}", &text[..20]);
        let (_, note) = text
            .split_once("\n\n[Showing lines 1001-3000 of 3000. Full output: ")
            .expect("truncation note");
        let path = note.strip_suffix(']').unwrap();
        let full = std::fs::read_to_string(path).unwrap();
        assert_eq!(full.lines().count(), 3000);
        std::fs::remove_file(path).unwrap();
    }
}
