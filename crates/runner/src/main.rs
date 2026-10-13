use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::bail;
use clap::{CommandFactory, Parser, Subcommand};
use telehand_runner::{RunExit, RunnerConfig, config};
use tokio_util::sync::CancellationToken;

mod menu;

#[derive(Parser)]
#[command(
    name = "telehand-runner",
    version,
    about = "Telehand runner",
    long_about = "Telehand runner. Run without a command for an interactive menu."
)]
struct Cli {
    /// Path of the runner config file.
    #[arg(long, global = true, env = "TELEHAND_RUNNER_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Save the server URL and key, and print the MCP URL for agents.
    Register { server_url: String, key: String },
    /// Connect to the server and serve tool calls.
    Run,
    /// Manage projects.
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
}

#[derive(Subcommand)]
enum ProjectCommand {
    /// Add a project with its main folder.
    Add {
        dir: PathBuf,
        /// Project name; defaults to the folder's name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Rename a project.
    Rename { name: String, new_name: String },
    /// Manage a project's extra folders (writable in addition to the main folder).
    Folder {
        #[command(subcommand)]
        command: FolderCommand,
    },
    /// List projects.
    List,
    /// Remove a project.
    #[command(visible_alias = "rm")]
    Remove { name: String },
}

#[derive(Subcommand)]
enum FolderCommand {
    /// Add an extra folder to a project.
    Add { name: String, dir: PathBuf },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_timer(tracing_subscriber::fmt::time::ChronoLocal::new(
            "%H:%M:%S".into(),
        ))
        .with_target(false)
        .init();
    // TLS for wss:// server URLs.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(config::default_path);

    let Some(command) = cli.command else {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return menu::run(&path).await;
        }
        // No one to answer the menu.
        Cli::command().print_help()?;
        std::process::exit(2);
    };
    let mut config = RunnerConfig::load(&path)?;
    match command {
        Command::Register { server_url, key } => {
            register(&mut config, &path, &server_url, &key).await?
        }
        Command::Run => {
            if !config.is_registered() {
                bail!("not registered; run `telehand-runner register <server_url> <key>` first");
            }
            run(config, &path).await?;
        }
        Command::Project { command } => match command {
            ProjectCommand::Add { dir, name } => {
                add_project(&mut config, &path, &dir, name.as_deref())?
            }
            ProjectCommand::Rename { name, new_name } => {
                rename_project(&mut config, &path, &name, &new_name)?
            }
            ProjectCommand::Folder {
                command: FolderCommand::Add { name, dir },
            } => add_folder(&mut config, &path, &name, &dir)?,
            ProjectCommand::List => {
                if config.projects.is_empty() {
                    println!(
                        "No projects. Add one with `telehand-runner project add <dir> [--name <name>]`."
                    );
                }
                print_projects(&config);
            }
            ProjectCommand::Remove { name } => remove_project(&mut config, &path, &name)?,
        },
    }
    Ok(())
}

// The commands, shared by the command line and the menu.

async fn register(
    config: &mut RunnerConfig,
    path: &Path,
    server_url: &str,
    key: &str,
) -> anyhow::Result<()> {
    let server_url = server_url.trim_end_matches('/').to_string();
    telehand_runner::check_key(&server_url, key).await?;
    config.server_url = server_url;
    config.key = key.to_string();
    config.save(path)?;
    println!("Registered. MCP URL for agents:");
    println!(
        "{}",
        telehand_proto::mcp_url(&config.server_url, &config.key)
    );
    Ok(())
}

/// Serve tool calls until Ctrl-C (or SIGTERM), or the server ends the connection.
async fn run(config: RunnerConfig, path: &Path) -> anyhow::Result<()> {
    // Full outputs of long commands left from earlier runs.
    telehand_tools::clean_task_output();
    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    let signal = tokio::spawn(async move {
        shutdown_signal().await;
        token.cancel();
    });
    // Project changes take effect without a restart; if the config
    // can't be watched, they take one.
    let (config, _watcher) = match telehand_runner::watch_config(path, config.clone()) {
        Ok((config, watcher)) => (config, Some(watcher)),
        Err(e) => {
            tracing::warn!(
                "cannot watch the config ({e:#}); restart the runner after changing projects"
            );
            (tokio::sync::watch::channel(config).1, None)
        }
    };
    let exit = telehand_runner::run(config, shutdown, |url| {
        println!("Connected. MCP URL for agents: {url}");
    })
    .await;
    // The menu may run the runner again.
    signal.abort();
    match exit? {
        RunExit::Shutdown => Ok(()),
        RunExit::Replaced => {
            bail!("another runner connected with the same key; this runner has been replaced")
        }
        RunExit::KeyRejected => bail!("the server rejected the key (unknown or removed)"),
    }
}

fn add_project(
    config: &mut RunnerConfig,
    path: &Path,
    dir: &Path,
    name: Option<&str>,
) -> anyhow::Result<()> {
    let project = config.add_project(dir, name)?.clone();
    config.save(path)?;
    println!(
        "Added project {} (main folder: {}).",
        project.name, project.main_folder
    );
    Ok(())
}

fn rename_project(
    config: &mut RunnerConfig,
    path: &Path,
    name: &str,
    new_name: &str,
) -> anyhow::Result<()> {
    config.rename_project(name, new_name)?;
    config.save(path)?;
    println!("Renamed project {name} to {new_name}.");
    Ok(())
}

fn add_folder(config: &mut RunnerConfig, path: &Path, name: &str, dir: &Path) -> anyhow::Result<()> {
    let project = config.add_folder(name, dir)?.clone();
    config.save(path)?;
    println!(
        "Added folder {} to project {}.",
        project.extra_folders.last().expect("just added"),
        project.name
    );
    Ok(())
}

fn print_projects(config: &RunnerConfig) {
    for project in &config.projects {
        println!("{}", project.name);
        println!("  main folder: {}", project.main_folder);
        if !project.extra_folders.is_empty() {
            println!("  extra folders: {}", project.extra_folders.join(", "));
        }
    }
}

fn remove_project(config: &mut RunnerConfig, path: &Path, name: &str) -> anyhow::Result<()> {
    config.remove_project(name)?;
    config.save(path)?;
    println!("Removed project {name}.");
    Ok(())
}

/// Ctrl-C, or SIGTERM on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c().await;
}

/// Resolves on Ctrl-C. Terminals without a console (e.g. Git Bash's mintty on
/// Windows) cannot deliver Ctrl-C; then this never resolves instead of
/// shutting the runner down at once.
async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %e, "cannot listen for Ctrl-C; close the window to stop the runner");
        std::future::pending::<()>().await;
    }
}
