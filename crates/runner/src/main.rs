use std::path::PathBuf;

use anyhow::bail;
use clap::{Parser, Subcommand};
use telehand_runner::{RunExit, RunnerConfig, config};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "telehand-runner", version, about = "Telehand runner")]
struct Cli {
    /// Path of the runner config file.
    #[arg(long, global = true, env = "TELEHAND_RUNNER_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
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

const RESTART_HINT: &str = "Restart `telehand-runner run` for the change to take effect.";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    // TLS for wss:// server URLs.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(config::default_path);
    let mut config = RunnerConfig::load(&path)?;

    match cli.command {
        Command::Register { server_url, key } => {
            let server_url = server_url.trim_end_matches('/').to_string();
            telehand_runner::check_key(&server_url, &key).await?;
            config.server_url = server_url;
            config.key = key;
            config.save(&path)?;
            println!("Registered. MCP URL for agents:");
            println!(
                "{}",
                telehand_proto::mcp_url(&config.server_url, &config.key)
            );
        }
        Command::Run => {
            if !config.is_registered() {
                bail!("not registered; run `telehand-runner register <server_url> <key>` first");
            }
            let shutdown = CancellationToken::new();
            let token = shutdown.clone();
            tokio::spawn(async move {
                shutdown_signal().await;
                token.cancel();
            });
            let exit = telehand_runner::run(config, shutdown, |url| {
                println!("Connected. MCP URL for agents: {url}");
            })
            .await?;
            match exit {
                RunExit::Shutdown => {}
                RunExit::Replaced => {
                    bail!(
                        "another runner connected with the same key; this runner has been replaced"
                    )
                }
                RunExit::KeyRejected => bail!("the server rejected the key (unknown or removed)"),
            }
        }
        Command::Project { command } => match command {
            ProjectCommand::Add { dir, name } => {
                let project = config.add_project(&dir, name.as_deref())?.clone();
                config.save(&path)?;
                println!(
                    "Added project {} (main folder: {}). {RESTART_HINT}",
                    project.name, project.main_folder
                );
            }
            ProjectCommand::Rename { name, new_name } => {
                config.rename_project(&name, &new_name)?;
                config.save(&path)?;
                println!("Renamed project {name} to {new_name}. {RESTART_HINT}");
            }
            ProjectCommand::Folder {
                command: FolderCommand::Add { name, dir },
            } => {
                let project = config.add_folder(&name, &dir)?.clone();
                config.save(&path)?;
                println!(
                    "Added folder {} to project {}. {RESTART_HINT}",
                    project.extra_folders.last().expect("just added"),
                    project.name
                );
            }
            ProjectCommand::List => {
                if config.projects.is_empty() {
                    println!(
                        "No projects. Add one with `telehand-runner project add <dir> [--name <name>]`."
                    );
                }
                for project in &config.projects {
                    println!("{}", project.name);
                    println!("  main folder: {}", project.main_folder);
                    if !project.extra_folders.is_empty() {
                        println!("  extra folders: {}", project.extra_folders.join(", "));
                    }
                }
            }
            ProjectCommand::Remove { name } => {
                config.remove_project(&name)?;
                config.save(&path)?;
                println!("Removed project {name}. {RESTART_HINT}");
            }
        },
    }
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
