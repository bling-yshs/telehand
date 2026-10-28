//! The interactive menu, shown when the server is started without a command
//! in a terminal. Esc goes back; Ctrl-C leaves the menu.

use std::fmt;
use std::net::SocketAddr;
use std::path::Path;

use inquire::validator::Validation;
use inquire::{Confirm, CustomUserError, InquireError, Select, Text};
use telehand_server::{admin, keys::KeyEntry};

pub async fn run(data_dir: &Path) -> anyhow::Result<()> {
    match main_menu(data_dir).await {
        Err(e) if matches!(
            e.downcast_ref::<InquireError>(),
            Some(InquireError::OperationInterrupted)
        ) => Ok(()),
        result => result,
    }
}

#[derive(Clone, Copy)]
enum MainChoice {
    Serve,
    CreateKey,
    ListKeys,
    RemoveKey,
    Exit,
}

impl fmt::Display for MainChoice {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Serve => "Serve",
            Self::CreateKey => "Create a key",
            Self::ListKeys => "List keys",
            Self::RemoveKey => "Remove a key",
            Self::Exit => "Exit",
        })
    }
}

async fn main_menu(data_dir: &Path) -> anyhow::Result<()> {
    loop {
        let choices = vec![
            MainChoice::Serve,
            MainChoice::CreateKey,
            MainChoice::ListKeys,
            MainChoice::RemoveKey,
            MainChoice::Exit,
        ];
        let choice = Select::new("Telehand server", choices)
            .with_help_message("↑↓ to move, Enter to choose, Esc to quit")
            .prompt();
        let result = match answer(choice)? {
            Some(MainChoice::Serve) => serve(data_dir).await,
            Some(MainChoice::CreateKey) => create_key(data_dir).await,
            Some(MainChoice::ListKeys) => list_keys(data_dir).await,
            Some(MainChoice::RemoveKey) => remove_key(data_dir).await,
            Some(MainChoice::Exit) | None => return Ok(()),
        };
        show_error(result)?;
    }
}

async fn serve(data_dir: &Path) -> anyhow::Result<()> {
    let validator = |input: &str| {
        Ok::<_, CustomUserError>(match input.trim().parse::<SocketAddr>() {
            Ok(_) => Validation::Valid,
            Err(_) => Validation::Invalid(
                format!("enter an address like {}", crate::DEFAULT_LISTEN).into(),
            ),
        })
    };
    let listen = Text::new("Listen address:")
        .with_initial_value(crate::DEFAULT_LISTEN)
        .with_validator(validator)
        .prompt();
    let Some(listen) = answer(listen)? else {
        return Ok(());
    };
    let listen = listen.trim().parse()?;
    println!("Press Ctrl-C to stop the server and return to the menu.");
    crate::serve(data_dir, listen).await?;
    println!("Server stopped.");
    Ok(())
}

async fn create_key(data_dir: &Path) -> anyhow::Result<()> {
    let name = Text::new("Note (optional):")
        .with_help_message("Tells keys apart, e.g. the runner machine's name; Enter to skip")
        .prompt();
    let Some(name) = answer(name)? else {
        return Ok(());
    };
    let name = Some(name.trim().to_string()).filter(|name| !name.is_empty());
    crate::create_key(data_dir, name).await?;
    println!("Register a runner with this key.");
    Ok(())
}

async fn list_keys(data_dir: &Path) -> anyhow::Result<()> {
    let keys = admin::list_keys(data_dir).await?;
    if keys.is_empty() {
        println!("No keys yet.");
    }
    crate::print_keys(&keys);
    Ok(())
}

async fn remove_key(data_dir: &Path) -> anyhow::Result<()> {
    let keys = admin::list_keys(data_dir).await?;
    if keys.is_empty() {
        println!("No keys yet.");
        return Ok(());
    }
    let choices = keys.iter().map(|(key, entry)| KeyItem(key, entry)).collect();
    let choice = Select::new("Remove which key?", choices)
        .with_help_message("↑↓ to move, Enter to choose, type to filter, Esc to go back")
        .prompt();
    let Some(KeyItem(key, _)) = answer(choice)? else {
        return Ok(());
    };
    let sure = Confirm::new(&format!(
        "Remove key {key}? Its runner is disconnected and its MCP URL stops working."
    ))
    .with_default(false)
    .prompt();
    if answer(sure)? != Some(true) {
        return Ok(());
    }
    crate::remove_key(data_dir, key).await
}

/// A key shown with its note and creation time.
struct KeyItem<'a>(&'a str, &'a KeyEntry);

impl fmt::Display for KeyItem<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}  ({}, created_at={})",
            self.0,
            self.1.name.as_deref().unwrap_or("-"),
            self.1.created_at
        )
    }
}

/// Esc (`None`) goes back; other prompt errors, Ctrl-C included, end the menu.
fn answer<T>(result: Result<T, InquireError>) -> anyhow::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Shows a failed action's error so the menu can go on; prompt errors end it.
fn show_error(result: anyhow::Result<()>) -> anyhow::Result<()> {
    match result {
        Err(e) if !e.is::<InquireError>() => {
            eprintln!("Error: {e:#}");
            Ok(())
        }
        result => result,
    }
}
