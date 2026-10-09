//! The `bash` tool, following pi's `bash.ts`: runs a command in the project's
//! main folder and returns stdout and stderr combined. Long output is cut: a
//! successful command shows the start of it, a failed one the start and the
//! end, and the full output is saved to a file under the task output folder,
//! whose path is shown.
//!
//! Unlike pi, a command still running after `wait` seconds (default 50) keeps
//! running as a task: `bash` returns its task ID, `bash_result` waits for its
//! result and `bash_kill` stops it. Every call stays short, whatever the
//! agent's request timeout, and a command survives the runner reconnecting.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use serde::Deserialize;
use serde_json::Value;
use telehand_proto::ToolOutput;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, watch},
    task::JoinHandle,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    Project,
    shell::{self, ShellConfig},
    truncate::truncate_tail,
};

const MAX_TIMEOUT_SECONDS: f64 = 2_147_483.647;
/// After the shell exits, keep reading until its pipes stay quiet this long:
/// background processes it started may hold them open.
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);
/// A successful command's output is returned in full up to this many
/// characters; beyond that, only its first `SUCCESS_HEAD_CHARS`.
const SUCCESS_MAX_CHARS: usize = 30_000;
const SUCCESS_HEAD_CHARS: usize = 2_000;
/// A failed command's output is returned in full up to this many characters;
/// beyond that, only its first and last `FAILURE_EDGE_CHARS`.
const FAILURE_MAX_CHARS: usize = 10_000;
const FAILURE_EDGE_CHARS: usize = 5_000;
/// Bytes that hold at least `FAILURE_EDGE_CHARS` characters (UTF-8 takes at
/// most 4 bytes per character), kept from the output's start and end.
const EDGE_BYTES: usize = FAILURE_EDGE_CHARS * 4;
/// Length of the random name of a task output file.
const OUTPUT_ID_LENGTH: u16 = 9;
/// Task output files older than this are deleted when the runner starts.
const OUTPUT_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How much of the latest output a still-running task shows.
const RECENT_LINES: usize = 20;
const RECENT_BYTES: usize = 4 * 1024;
/// How long `bash_kill` waits for a killed command to end.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// Commands that outlived the call that started them, until their result is taken.
static TASKS: LazyLock<Mutex<HashMap<String, Arc<Task>>>> = LazyLock::new(Default::default);
static NEXT_TASK: AtomicU64 = AtomicU64::new(1);

#[derive(Deserialize)]
struct Args {
    command: String,
    #[serde(default)]
    timeout: Option<f64>,
    #[serde(default)]
    wait: Option<f64>,
}

#[derive(Deserialize)]
struct TaskArgs {
    task_id: String,
    #[serde(default)]
    wait: Option<f64>,
}

/// A running (or finished, not yet reported) command.
struct Task {
    id: String,
    started: Instant,
    /// The `timeout` argument, for the timeout message.
    timeout: Option<f64>,
    output: Mutex<Output>,
    kill: CancellationToken,
    ending: watch::Receiver<Option<Ending>>,
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
    let wait = match resolve_wait(args.wait) {
        Ok(wait) => wait,
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

    let task = start(shell, args.command, cwd.clone(), args.timeout, timeout);
    // Cancelling the call (the agent gave up on it) kills the command.
    match wait_for(&task, wait, cancel, true).await {
        Some(ending) => report(&task, &ending),
        None => {
            TASKS.lock().unwrap().insert(task.id.clone(), task.clone());
            ToolOutput::text(still_running(&task))
        }
    }
}

/// `bash_result`: wait for a task and report its result once it has ended.
pub async fn result(args: Value, cancel: &CancellationToken) -> ToolOutput {
    let args: TaskArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for bash_result: {e}")),
    };
    let wait = match resolve_wait(args.wait) {
        Ok(wait) => wait,
        Err(message) => return ToolOutput::error(message),
    };
    let Some(task) = find_task(&args.task_id) else {
        return unknown_task(&args.task_id);
    };
    // Cancelling this call only stops waiting.
    match wait_for(&task, wait, cancel, false).await {
        Some(ending) => {
            TASKS.lock().unwrap().remove(&task.id);
            report(&task, &ending)
        }
        None => ToolOutput::text(still_running(&task)),
    }
}

/// `bash_kill`: kill a task and report its output.
pub async fn kill(args: Value) -> ToolOutput {
    let args: TaskArgs = match serde_json::from_value(args) {
        Ok(args) => args,
        Err(e) => return ToolOutput::error(format!("Invalid arguments for bash_kill: {e}")),
    };
    let Some(task) = find_task(&args.task_id) else {
        return unknown_task(&args.task_id);
    };
    task.kill.cancel();
    let Some(ending) = wait_for(&task, KILL_WAIT, &CancellationToken::new(), false).await else {
        return ToolOutput::error(format!(
            "Task {} did not stop within {} seconds.",
            task.id,
            KILL_WAIT.as_secs()
        ));
    };
    TASKS.lock().unwrap().remove(&task.id);
    let (text, is_error) = outcome(&task, &ending);
    match ending {
        Ending::Aborted => ToolOutput::text(format!("Killed task {}.\n\n{text}", task.id)),
        _ => ToolOutput {
            is_error,
            ..ToolOutput::text(text)
        },
    }
}

fn find_task(id: &str) -> Option<Arc<Task>> {
    TASKS.lock().unwrap().get(id).cloned()
}

fn unknown_task(id: &str) -> ToolOutput {
    ToolOutput::error(format!(
        "Unknown task ID: {id}. Its result may already have been returned, or the runner was restarted."
    ))
}

fn resolve_wait(seconds: Option<f64>) -> Result<Duration, String> {
    let seconds = seconds.unwrap_or(telehand_proto::tool_defs::BASH_DEFAULT_WAIT_SECONDS);
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("Invalid wait: must be a non-negative number of seconds".to_string());
    }
    Duration::try_from_secs_f64(seconds).map_err(|e| format!("Invalid wait: {e}"))
}

/// Start the command in the background.
fn start(
    shell: ShellConfig,
    command: String,
    cwd: PathBuf,
    timeout_seconds: Option<f64>,
    timeout: Option<Duration>,
) -> Arc<Task> {
    let (ending_tx, ending) = watch::channel(None);
    let task = Arc::new(Task {
        id: format!("bash-{}", NEXT_TASK.fetch_add(1, Ordering::Relaxed)),
        started: Instant::now(),
        timeout: timeout_seconds,
        output: Mutex::new(Output::new()),
        kill: CancellationToken::new(),
        ending,
    });
    let job = task.clone();
    tokio::spawn(async move {
        let ending = execute(&shell, &command, &cwd, timeout, &job.kill, &job.output)
            .await
            .unwrap_or_else(Ending::Failed);
        let _ = ending_tx.send(Some(ending));
    });
    task
}

/// Wait up to `wait` for the task to end. When `cancel` fires, kill the task
/// and wait for it if `kill_on_cancel`, else stop waiting.
async fn wait_for(
    task: &Task,
    wait: Duration,
    cancel: &CancellationToken,
    kill_on_cancel: bool,
) -> Option<Ending> {
    let mut ending = task.ending.clone();
    let ended = async move {
        match ending.wait_for(Option::is_some).await {
            Ok(ending) => (*ending).clone().expect("waited for an ending"),
            Err(_) => Ending::Failed("The command's task stopped unexpectedly.".to_string()),
        }
    };
    tokio::pin!(ended);
    tokio::select! {
        ending = &mut ended => Some(ending),
        _ = tokio::time::sleep(wait) => None,
        _ = cancel.cancelled() => {
            if kill_on_cancel {
                task.kill.cancel();
                Some(ended.await)
            } else {
                None
            }
        }
    }
}

fn still_running(task: &Task) -> String {
    let recent = task.output.lock().unwrap().recent();
    let latest = if recent.is_empty() {
        "No output yet.".to_string()
    } else {
        format!("Latest output:\n{recent}")
    };
    format!(
        "Command is still running after {} seconds. Task ID: {}\nCall bash_result with this task ID to wait for its result, or bash_kill to stop it.\n\n{latest}",
        task.started.elapsed().as_secs(),
        task.id
    )
}

fn report(task: &Task, ending: &Ending) -> ToolOutput {
    let (text, is_error) = outcome(task, ending);
    if is_error {
        ToolOutput::error(text)
    } else {
        ToolOutput::text(text)
    }
}

/// The result text of an ended command (as pi words it) and whether it failed.
fn outcome(task: &Task, ending: &Ending) -> (String, bool) {
    let mut output = task.output.lock().unwrap();
    match ending {
        Ending::Aborted => (
            append_status(&output.format("", false), "Command aborted"),
            true,
        ),
        Ending::TimedOut => (
            append_status(
                &output.format("", false),
                &format!(
                    "Command timed out after {} seconds",
                    task.timeout.unwrap_or_default()
                ),
            ),
            true,
        ),
        Ending::Exited(0) => (output.format("(no output)", true), false),
        Ending::Exited(code) => (
            append_status(
                &output.format("(no output)", false),
                &format!("Command exited with code {code}"),
            ),
            true,
        ),
        Ending::Failed(message) => (message.clone(), true),
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

#[derive(Debug, Clone)]
enum Ending {
    Exited(i32),
    TimedOut,
    Aborted,
    /// The command could not be run.
    Failed(String),
}

/// Run the command, collecting its output, until it exits, times out or `kill` fires.
async fn execute(
    shell: &ShellConfig,
    command: &str,
    cwd: &Path,
    timeout: Option<Duration>,
    kill: &CancellationToken,
    output: &Mutex<Output>,
) -> Result<Ending, String> {
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

    let far_future = Instant::now() + Duration::from_secs(86400 * 365 * 30);
    let deadline = tokio::time::sleep_until(timeout.map_or(far_future, |t| Instant::now() + t));
    tokio::pin!(deadline);
    let mut timed_out = false;
    let mut aborted = false;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            Some(chunk) = rx.recv() => output.lock().unwrap().append(&chunk),
            _ = &mut deadline, if timeout.is_some() && !timed_out => {
                timed_out = true;
                kill_tree(pid);
            }
            _ = kill.cancelled(), if !aborted => {
                aborted = true;
                kill_tree(pid);
            }
        }
    };
    // Read what is left, without waiting on pipes that background processes keep open.
    while let Ok(Some(chunk)) = tokio::time::timeout(EXIT_STDIO_GRACE, rx.recv()).await {
        output.lock().unwrap().append(&chunk);
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
    Ok(ending)
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
        let taskkill = Path::new(&system_root)
            .join("System32")
            .join("taskkill.exe");
        let _ = std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// The folder full outputs of long commands are saved in.
fn task_output_dir() -> PathBuf {
    std::env::temp_dir().join("telehand").join("task-output")
}

/// Delete task output files older than three days, left from earlier runs.
pub fn clean_task_output() {
    clean_old_files(&task_output_dir(), OUTPUT_MAX_AGE);
}

fn clean_old_files(dir: &Path, max_age: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if metadata.is_file() && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Collects streamed output with bounded memory: the start and the end of the
/// output stay in memory, and once the output is longer than any command may
/// return in full, all of it goes to a file.
struct Output {
    /// The first `EDGE_BYTES` bytes.
    head: Vec<u8>,
    /// At least the last `EDGE_BYTES` bytes, starting at a character boundary.
    tail: Vec<u8>,
    total_chars: usize,
    /// Output not yet in the file (all of it while there is no file).
    buffered: Vec<u8>,
    file: Option<(PathBuf, File)>,
    file_error: Option<String>,
}

impl Output {
    fn new() -> Self {
        Self {
            head: Vec::new(),
            tail: Vec::new(),
            total_chars: 0,
            buffered: Vec::new(),
            file: None,
            file_error: None,
        }
    }

    fn append(&mut self, data: &[u8]) {
        // UTF-8 continuation bytes do not start a character.
        self.total_chars += data.iter().filter(|&&b| (b & 0xC0) != 0x80).count();
        let room = EDGE_BYTES.saturating_sub(self.head.len());
        self.head.extend_from_slice(&data[..room.min(data.len())]);
        self.tail.extend_from_slice(data);
        if self.tail.len() > EDGE_BYTES * 2 {
            self.trim_tail();
        }

        if self.file.is_none() && self.total_chars <= SUCCESS_MAX_CHARS {
            self.buffered.extend_from_slice(data);
            return;
        }
        self.persist();
        self.write_to_file(data);
    }

    /// The last lines so far, for a command that is still running.
    fn recent(&self) -> String {
        let text = String::from_utf8_lossy(&self.tail);
        truncate_tail(text.trim_end_matches('\n'), RECENT_LINES, RECENT_BYTES).content
    }

    fn trim_tail(&mut self) {
        let mut start = self.tail.len() - EDGE_BYTES;
        while start < self.tail.len() && (self.tail[start] & 0xC0) == 0x80 {
            start += 1;
        }
        self.tail.drain(..start);
    }

    /// Move the output into a file, if not done yet.
    fn persist(&mut self) {
        if self.file.is_some() || self.file_error.is_some() {
            return;
        }
        let dir = task_output_dir();
        let path = dir.join(format!(
            "{}.output",
            cuid2::CuidConstructor::new()
                .with_length(OUTPUT_ID_LENGTH)
                .create_id()
        ));
        let created = fs::create_dir_all(&dir).and_then(|()| {
            File::options()
                .write(true)
                .create_new(true)
                .open(&path)
        });
        match created {
            Ok(file) => {
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

    /// The text for the agent: all of the output, or for long output its start
    /// (and, if the command failed, its end) with where the full output is.
    /// `empty` stands in for no output.
    fn format(&mut self, empty: &str, succeeded: bool) -> String {
        let max_chars = if succeeded {
            SUCCESS_MAX_CHARS
        } else {
            FAILURE_MAX_CHARS
        };
        if self.total_chars <= max_chars {
            // Within the limits, all of the output is still buffered.
            let text = String::from_utf8_lossy(&self.buffered);
            return if text.is_empty() {
                empty.to_string()
            } else {
                text.into_owned()
            };
        }
        self.persist();
        let full_output = match (&self.file, &self.file_error) {
            (Some((path, _)), None) => path.display().to_string(),
            (_, Some(error)) => format!("(could not save: {error})"),
            (None, None) => "(could not save)".to_string(),
        };
        let note = format!("[Output truncated. Full output: {full_output}]");
        let head = String::from_utf8_lossy(&self.head);
        if succeeded {
            format!("{}\n\n{note}", first_chars(&head, SUCCESS_HEAD_CHARS))
        } else {
            let tail = String::from_utf8_lossy(&self.tail);
            format!(
                "{}\n\n{note}\n\n{}",
                first_chars(&head, FAILURE_EDGE_CHARS),
                last_chars(&tail, FAILURE_EDGE_CHARS)
            )
        }
    }
}

fn first_chars(text: &str, count: usize) -> &str {
    text.char_indices()
        .nth(count)
        .map_or(text, |(end, _)| &text[..end])
}

fn last_chars(text: &str, count: usize) -> &str {
    match count.checked_sub(1) {
        None => "",
        Some(skip) => text
            .char_indices()
            .rev()
            .nth(skip)
            .map_or(text, |(start, _)| &text[start..]),
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
        assert_eq!(text(&out), "start\n\n\nCommand timed out after 0.5 seconds");

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
        let out = run(
            &project(dir.path()),
            json!({"command": "sleep 10"}),
            &cancel,
        )
        .await;
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

    /// Split off the truncation note, returning the text around it and the
    /// saved full output (deleting its file).
    fn split_note(text: &str) -> (&str, &str, String) {
        let (before, rest) = text
            .split_once("\n\n[Output truncated. Full output: ")
            .expect("truncation note");
        let (path, after) = rest.split_once(']').unwrap();
        assert!(
            Path::new(path).starts_with(task_output_dir()),
            "{path}"
        );
        let name = Path::new(path).file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), "123456789.output".len(), "{name}");
        assert!(name.ends_with(".output"), "{name}");
        let full = std::fs::read_to_string(path).unwrap();
        std::fs::remove_file(path).unwrap();
        (before, after, full)
    }

    #[tokio::test]
    async fn long_successful_output_keeps_the_start_and_saves_all() {
        let dir = tempfile::tempdir().unwrap();
        // 30,000 characters of two bytes each fit; one more character does not.
        let out = bash(
            dir.path(),
            json!({"command": "printf 'é%.0s' $(seq 30000)"}),
        )
        .await;
        assert_eq!(text(&out), "é".repeat(30_000));

        let out = bash(
            dir.path(),
            json!({"command": "printf 'é%.0s' $(seq 30001)"}),
        )
        .await;
        assert!(!out.is_error);
        let (head, after, full) = split_note(text(&out));
        assert_eq!(head, "é".repeat(2_000));
        assert_eq!(after, "");
        assert_eq!(full, "é".repeat(30_001));
    }

    #[tokio::test]
    async fn long_failing_output_keeps_the_start_and_end_and_saves_all() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(
            dir.path(),
            json!({"command": "printf 'a%.0s' $(seq 10000); exit 2"}),
        )
        .await;
        assert_eq!(
            text(&out),
            format!("{}\n\nCommand exited with code 2", "a".repeat(10_000))
        );

        let out = bash(
            dir.path(),
            json!({"command": "printf 'a%.0s' $(seq 5000); printf 'b%.0s' $(seq 1001); printf 'c%.0s' $(seq 5000); exit 2"}),
        )
        .await;
        assert!(out.is_error);
        let (head, after, full) = split_note(text(&out));
        assert_eq!(head, "a".repeat(5_000));
        assert_eq!(
            after,
            format!("\n\n{}\n\nCommand exited with code 2", "c".repeat(5_000))
        );
        assert_eq!(
            full,
            format!("{}{}{}", "a".repeat(5_000), "b".repeat(1_001), "c".repeat(5_000))
        );
    }

    #[test]
    fn old_task_output_is_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.output");
        let new = dir.path().join("new.output");
        File::create(&old)
            .unwrap()
            .set_modified(SystemTime::now() - OUTPUT_MAX_AGE - Duration::from_secs(60))
            .unwrap();
        File::create(&new).unwrap();
        clean_old_files(dir.path(), OUTPUT_MAX_AGE);
        assert!(!old.exists());
        assert!(new.exists());
    }

    fn task_id(text: &str) -> String {
        let (_, rest) = text.split_once("Task ID: ").expect("task ID");
        rest.lines().next().unwrap().to_string()
    }

    #[tokio::test]
    async fn slow_commands_continue_as_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(
            dir.path(),
            json!({"command": "echo started; sleep 1; echo finished", "wait": 0.3}),
        )
        .await;
        assert!(!out.is_error, "{}", text(&out));
        assert!(
            text(&out).starts_with("Command is still running after 0 seconds. Task ID: bash-"),
            "{}",
            text(&out)
        );
        assert!(
            text(&out).ends_with("Latest output:\nstarted"),
            "{}",
            text(&out)
        );
        let id = task_id(text(&out));

        let cancel = CancellationToken::new();
        let out = result(json!({"task_id": id, "wait": 10}), &cancel).await;
        assert_eq!(text(&out), "started\nfinished\n");

        let out = result(json!({"task_id": id}), &cancel).await;
        assert!(text(&out).starts_with("Unknown task ID"), "{}", text(&out));
    }

    #[tokio::test]
    async fn waiting_again_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(
            dir.path(),
            json!({"command": "sleep 1; echo done", "wait": 0}),
        )
        .await;
        assert!(text(&out).ends_with("No output yet."), "{}", text(&out));
        let id = task_id(text(&out));

        let cancel = CancellationToken::new();
        let out = result(json!({"task_id": id, "wait": 0.1}), &cancel).await;
        assert!(
            text(&out).contains(&format!("Task ID: {id}")),
            "{}",
            text(&out)
        );
        let out = result(json!({"task_id": id, "wait": 10}), &cancel).await;
        assert_eq!(text(&out), "done\n");
    }

    #[tokio::test]
    async fn tasks_can_be_killed() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(
            dir.path(),
            json!({"command": "echo hi; sleep 30", "wait": 0.3}),
        )
        .await;
        let id = task_id(text(&out));

        let started = Instant::now();
        let out = kill(json!({"task_id": id})).await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!out.is_error);
        assert_eq!(
            text(&out),
            format!("Killed task {id}.\n\nhi\n\n\nCommand aborted")
        );
        assert!(kill(json!({"task_id": id})).await.is_error);
    }

    #[tokio::test]
    async fn wait_must_be_non_negative() {
        let dir = tempfile::tempdir().unwrap();
        let out = bash(dir.path(), json!({"command": "true", "wait": -1})).await;
        assert_eq!(
            text(&out),
            "Invalid wait: must be a non-negative number of seconds"
        );
    }
}
