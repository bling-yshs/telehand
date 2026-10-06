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
            telehand_proto::ws_url(&server_url).map_err(anyhow::Error::msg)?;
            config.server_url = server_url.trim_end_matches('/').to_string();
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
                let _ = tokio::signal::ctrl_c().await;
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
            ProjectCommand::Add { name, dir } => {
                let project = config.add_project(&name, &dir)?.clone();
                config.save(&path)?;
                println!(
                    "Added project {} (main folder: {}). {RESTART_HINT}",
                    project.name, project.main_folder
                );
            }
        },
    }
    Ok(())
}
