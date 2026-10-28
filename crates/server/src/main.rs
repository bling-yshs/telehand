use std::io::IsTerminal;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use clap::{CommandFactory, Parser, Subcommand};
use telehand_server::{ServeOptions, admin, keys::Keys};

mod menu;

const DEFAULT_LISTEN: &str = "0.0.0.0:20250";

#[derive(Parser)]
#[command(
    name = "telehand-server",
    version,
    about = "Telehand server",
    long_about = "Telehand server. Run without a command for an interactive menu."
)]
struct Cli {
    /// Directory holding keys.json and admin.sock.
    #[arg(
        long,
        global = true,
        env = "TELEHAND_DATA_DIR",
        default_value = "./data"
    )]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server.
    Serve {
        /// Address to listen on.
        #[arg(long, default_value = DEFAULT_LISTEN)]
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
    #[command(visible_aliases = ["new", "add"])]
    Create {
        /// A note to tell keys apart.
        #[arg(long)]
        name: Option<String>,
    },
    /// List keys.
    List,
    /// Remove a key; its runner is disconnected and its MCP URL stops working.
    #[command(visible_alias = "rm")]
    Remove { key: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let Cli { data_dir, command } = Cli::parse();
    let Some(command) = command else {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return menu::run(&data_dir).await;
        }
        // No one to answer the menu.
        Cli::command().print_help()?;
        std::process::exit(2);
    };
    match command {
        Command::Serve { listen } => serve(&data_dir, listen).await,
        Command::Key { command } => match command {
            KeyCommand::Create { name } => create_key(&data_dir, name).await,
            KeyCommand::List => {
                print_keys(&admin::list_keys(&data_dir).await?);
                Ok(())
            }
            KeyCommand::Remove { key } => remove_key(&data_dir, &key).await,
        },
    }
}

// The commands, shared by the command line and the menu.

/// Serve until Ctrl-C (or SIGTERM).
async fn serve(data_dir: &Path, listen: SocketAddr) -> anyhow::Result<()> {
    let server = telehand_server::start(ServeOptions {
        listen,
        data_dir: data_dir.to_path_buf(),
    })
    .await?;
    tracing::info!(addr = %server.addr, "listening");
    let token = server.shutdown_token();
    let signal = tokio::spawn(async move {
        shutdown_signal().await;
        token.cancel();
    });
    let result = server.wait().await;
    // The menu may serve again.
    signal.abort();
    result
}

async fn create_key(data_dir: &Path, name: Option<String>) -> anyhow::Result<()> {
    let key = admin::create_key(data_dir, name).await?;
    println!("{key}");
    Ok(())
}

fn print_keys(keys: &Keys) {
    for (key, entry) in keys {
        println!(
            "{key}\t{}\tcreated_at={}",
            entry.name.as_deref().unwrap_or("-"),
            entry.created_at
        );
    }
}

async fn remove_key(data_dir: &Path, key: &str) -> anyhow::Result<()> {
    admin::remove_key(data_dir, key).await?;
    println!("Removed key {key}");
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

/// Resolves on Ctrl-C; never resolves if Ctrl-C cannot be listened for,
/// rather than shutting down at once.
async fn ctrl_c() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(error = %e, "cannot listen for Ctrl-C");
        std::future::pending::<()>().await;
    }
}
