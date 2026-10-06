use std::{net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand};
use telehand_server::{ServeOptions, admin};

#[derive(Parser)]
#[command(name = "telehand-server", version, about = "Telehand server")]
struct Cli {
    /// Directory holding keys.json and admin.sock.
    #[arg(long, global = true, env = "TELEHAND_DATA_DIR", default_value = "./data")]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server.
    Serve {
        /// Address to listen on.
        #[arg(long, default_value = "0.0.0.0:8080")]
        listen: SocketAddr,
    },
    /// Manage keys.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Create a key.
    Create {
        /// A note to tell keys apart.
        #[arg(long)]
        name: Option<String>,
    },
    /// List keys.
    List,
    /// Remove a key; its runner is disconnected and its MCP URL stops working.
    Rm {
        key: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let Cli { data_dir, command } = Cli::parse();
    match command {
        Command::Serve { listen } => {
            let server = telehand_server::start(ServeOptions { listen, data_dir }).await?;
            tracing::info!(addr = %server.addr, "listening");
            let token = server.shutdown_token();
            tokio::spawn(async move {
                shutdown_signal().await;
                token.cancel();
            });
            server.wait().await
        }
        Command::Key { command } => match command {
            KeyCommand::Create { name } => {
                let key = admin::create_key(&data_dir, name).await?;
                println!("{key}");
                Ok(())
            }
            KeyCommand::List => {
                for (key, entry) in admin::list_keys(&data_dir).await? {
                    println!(
                        "{key}\t{}\tcreated_at={}",
                        entry.name.as_deref().unwrap_or("-"),
                        entry.created_at
                    );
                }
                Ok(())
            }
            KeyCommand::Rm { key } => {
                admin::remove_key(&data_dir, &key).await?;
                println!("Removed key {key}");
                Ok(())
            }
        },
    }
}

/// Ctrl-C, or SIGTERM on Unix.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
