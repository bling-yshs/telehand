//! The interactive menu, shown when the runner is started without a command
//! in a terminal. Esc goes back a level; Ctrl-C leaves the menu.

use std::fmt;
use std::path::{MAIN_SEPARATOR, Path, PathBuf, is_separator};

use anyhow::bail;
use inquire::autocompletion::Replacement;
use inquire::validator::Validation;
use inquire::{Autocomplete, Confirm, CustomUserError, InquireError, Select, Text};
use telehand_proto::ProjectInfo;
use telehand_runner::{RunnerConfig, config};

pub async fn run(path: &Path) -> anyhow::Result<()> {
    match main_menu(path).await {
        Err(e) if matches!(
            e.downcast_ref::<InquireError>(),
            Some(InquireError::OperationInterrupted)
        ) => Ok(()),
        result => result,
    }
}

#[derive(Clone, Copy)]
enum MainChoice {
    Register,
    Run,
    Projects,
    Exit,
}

impl fmt::Display for MainChoice {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::Register => "Register with a server",
            Self::Run => "Run",
            Self::Projects => "Manage projects",
            Self::Exit => "Exit",
        })
    }
}

async fn main_menu(path: &Path) -> anyhow::Result<()> {
    loop {
        // Reloaded each time: the file may have changed meanwhile.
        let config = RunnerConfig::load(path)?;
        let (title, start) = if config.is_registered() {
            (format!("Telehand runner (server: {})", config.server_url), 1)
        } else {
            ("Telehand runner (not registered)".to_string(), 0)
        };
        let choices = vec![
            MainChoice::Register,
            MainChoice::Run,
            MainChoice::Projects,
            MainChoice::Exit,
        ];
        let choice = Select::new(&title, choices)
            .with_starting_cursor(start)
            .with_help_message("↑↓ to move, Enter to choose, Esc to quit")
            .prompt();
        let result = match answer(choice)? {
            Some(MainChoice::Register) => register(config, path).await,
            Some(MainChoice::Run) => run_runner(config, path).await,
            Some(MainChoice::Projects) => projects_menu(path),
            Some(MainChoice::Exit) | None => return Ok(()),
        };
        show_error(result)?;
    }
}

async fn register(mut config: RunnerConfig, path: &Path) -> anyhow::Result<()> {
    let server_url = Text::new("Server URL:")
        .with_initial_value(&config.server_url)
        .with_validator(required)
        .prompt();
    let Some(server_url) = answer(server_url)? else {
        return Ok(());
    };
    let key = Text::new("Key:")
        .with_initial_value(&config.key)
        .with_validator(required)
        .prompt();
    let Some(key) = answer(key)? else {
        return Ok(());
    };
    crate::register(&mut config, path, server_url.trim(), key.trim()).await
}

async fn run_runner(config: RunnerConfig, path: &Path) -> anyhow::Result<()> {
    if !config.is_registered() {
        bail!("not registered; choose \"{}\" first", MainChoice::Register);
    }
    println!("Press Ctrl-C to stop the runner and return to the menu.");
    crate::run(config, path).await?;
    println!("Runner stopped.");
    Ok(())
}

#[derive(Clone, Copy)]
enum ProjectChoice {
    List,
    Add,
    AddFolder,
    Rename,
    Remove,
    Back,
}

impl fmt::Display for ProjectChoice {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Self::List => "List projects",
            Self::Add => "Add a project",
            Self::AddFolder => "Add a folder to a project",
            Self::Rename => "Rename a project",
            Self::Remove => "Remove a project",
            Self::Back => "Back",
        })
    }
}

fn projects_menu(path: &Path) -> anyhow::Result<()> {
    loop {
        let mut config = RunnerConfig::load(path)?;
        let title = format!("Projects ({}):", config.projects.len());
        let choices = vec![
            ProjectChoice::List,
            ProjectChoice::Add,
            ProjectChoice::AddFolder,
            ProjectChoice::Rename,
            ProjectChoice::Remove,
            ProjectChoice::Back,
        ];
        let choice = Select::new(&title, choices)
            .with_help_message("↑↓ to move, Enter to choose, Esc to go back")
            .prompt();
        let result = match answer(choice)? {
            Some(ProjectChoice::List) => {
                if config.projects.is_empty() {
                    println!("No projects yet.");
                }
                crate::print_projects(&config);
                Ok(())
            }
            Some(ProjectChoice::Add) => add_project(&mut config, path),
            Some(ProjectChoice::AddFolder) => add_folder(&mut config, path),
            Some(ProjectChoice::Rename) => rename_project(&mut config, path),
            Some(ProjectChoice::Remove) => remove_project(&mut config, path),
            Some(ProjectChoice::Back) | None => return Ok(()),
        };
        show_error(result)?;
    }
}

fn add_project(config: &mut RunnerConfig, path: &Path) -> anyhow::Result<()> {
    let Some(dir) = ask_folder("Main folder:")? else {
        return Ok(());
    };
    let folder_name = Path::new(&dir)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(name) = ask_name(config, &folder_name, None)? else {
        return Ok(());
    };
    crate::add_project(config, path, Path::new(&dir), Some(&name))
}

fn add_folder(config: &mut RunnerConfig, path: &Path) -> anyhow::Result<()> {
    let Some(name) = pick_project(config, "Add a folder to which project?")? else {
        return Ok(());
    };
    let Some(dir) = ask_folder("Folder:")? else {
        return Ok(());
    };
    crate::add_folder(config, path, &name, Path::new(&dir))
}

fn rename_project(config: &mut RunnerConfig, path: &Path) -> anyhow::Result<()> {
    let Some(name) = pick_project(config, "Rename which project?")? else {
        return Ok(());
    };
    let Some(new_name) = ask_name(config, &name, Some(&name))? else {
        return Ok(());
    };
    if new_name == name {
        return Ok(());
    }
    crate::rename_project(config, path, &name, &new_name)
}

fn remove_project(config: &mut RunnerConfig, path: &Path) -> anyhow::Result<()> {
    let Some(name) = pick_project(config, "Remove which project?")? else {
        return Ok(());
    };
    let sure = Confirm::new(&format!(
        "Remove project {name}? Its folders stay on disk."
    ))
    .with_default(false)
    .prompt();
    if answer(sure)? != Some(true) {
        return Ok(());
    }
    crate::remove_project(config, path, &name)
}

/// A project shown with its main folder.
struct ProjectItem<'a>(&'a ProjectInfo);

impl fmt::Display for ProjectItem<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}  ({})", self.0.name, self.0.main_folder)
    }
}

/// The name of a project the user picks; `None` if there are none or on Esc.
fn pick_project(config: &RunnerConfig, message: &str) -> anyhow::Result<Option<String>> {
    if config.projects.is_empty() {
        println!("No projects yet.");
        return Ok(None);
    }
    let choices = config.projects.iter().map(ProjectItem).collect();
    let choice = Select::new(message, choices)
        .with_help_message("↑↓ to move, Enter to choose, type to filter, Esc to go back")
        .prompt();
    Ok(answer(choice)?.map(|project| project.0.name.clone()))
}

/// A name for a new project, or for the project now named `current`.
fn ask_name(
    config: &RunnerConfig,
    initial: &str,
    current: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let validator = move |input: &str| {
        let input = input.trim();
        Ok::<_, CustomUserError>(match config.check_new_name(input) {
            Err(e) if Some(input) != current => Validation::Invalid(format!("{e:#}").into()),
            _ => Validation::Valid,
        })
    };
    let name = Text::new("Project name:")
        .with_initial_value(initial)
        .with_validator(validator)
        .prompt();
    Ok(answer(name)?.map(|name| name.trim().to_string()))
}

/// An existing folder, as its canonical path.
fn ask_folder(message: &str) -> anyhow::Result<Option<String>> {
    let validator = |input: &str| {
        Ok::<_, CustomUserError>(if input.trim().is_empty() {
            Validation::Invalid("enter a folder path".into())
        } else {
            match config::canonical_dir(&folder_path(input)) {
                Ok(_) => Validation::Valid,
                Err(e) => Validation::Invalid(format!("{e:#}").into()),
            }
        })
    };
    let input = Text::new(message)
        .with_help_message("Tab completes folder names; a pasted path works too")
        .with_autocomplete(FolderCompleter)
        .with_validator(validator)
        .prompt();
    match answer(input)? {
        Some(input) => Ok(Some(config::canonical_dir(&folder_path(&input))?)),
        None => Ok(None),
    }
}

/// Typed or pasted paths; Windows' "Copy as path" adds quotes.
fn folder_path(input: &str) -> PathBuf {
    PathBuf::from(input.trim().trim_matches('"'))
}

fn required(input: &str) -> Result<Validation, CustomUserError> {
    Ok(if input.trim().is_empty() {
        Validation::Invalid("required".into())
    } else {
        Validation::Valid
    })
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

/// Tab-completes folder names in a path, like a shell.
#[derive(Clone)]
struct FolderCompleter;

impl Autocomplete for FolderCompleter {
    fn get_suggestions(&mut self, input: &str) -> Result<Vec<String>, CustomUserError> {
        Ok(subfolders(input))
    }

    fn get_completion(
        &mut self,
        input: &str,
        highlighted: Option<String>,
    ) -> Result<Replacement, CustomUserError> {
        if highlighted.is_some() {
            return Ok(highlighted);
        }
        let prefix = common_prefix(&subfolders(input));
        Ok(prefix.filter(|prefix| prefix.len() > input.len()))
    }
}

/// The folders in `input`'s parent whose names start with its last part,
/// ignoring case, each ending in a separator. Hidden folders only show once
/// the part starts with a dot.
fn subfolders(input: &str) -> Vec<String> {
    let input = input.trim_start_matches('"');
    let split = input.rfind(is_separator).map_or(0, |i| i + 1);
    let (parent, part) = input.split_at(split);
    if parent.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let part = part.to_lowercase();
    let mut folders: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.to_lowercase().starts_with(&part))
        .filter(|name| !name.starts_with('.') || part.starts_with('.'))
        .map(|name| format!("{parent}{name}{MAIN_SEPARATOR}"))
        .collect();
    folders.sort_by_key(|folder| folder.to_lowercase());
    folders
}

fn common_prefix(strings: &[String]) -> Option<String> {
    let (first, rest) = strings.split_first()?;
    let len = rest.iter().fold(first.len(), |len, s| {
        first[..len]
            .char_indices()
            .zip(s.chars())
            .find(|((_, a), b)| a != b)
            .map_or(len.min(s.len()), |((i, _), _)| i)
    });
    Some(first[..len].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_completer_completes_like_a_shell() {
        let tmp = tempfile::tempdir().unwrap();
        for dir in ["Alpha", "alps", "beta", "beth", ".hidden"] {
            std::fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        std::fs::write(tmp.path().join("alpine.txt"), "").unwrap();
        let parent = format!("{}{MAIN_SEPARATOR}", tmp.path().display());
        let at = |names: &[&str]| -> Vec<String> {
            names
                .iter()
                .map(|name| format!("{parent}{name}{MAIN_SEPARATOR}"))
                .collect()
        };
        let mut completer = FolderCompleter;
        let mut suggest = |input: &str| {
            completer
                .get_suggestions(&format!("{parent}{input}"))
                .unwrap()
        };

        // Case-insensitive; files and hidden folders left out.
        assert_eq!(suggest("al"), at(&["Alpha", "alps"]));
        assert_eq!(suggest(""), at(&["Alpha", "alps", "beta", "beth"]));
        assert_eq!(suggest("."), at(&[".hidden"]));
        assert!(suggest("x").is_empty());
        assert_eq!(suggest("alpha"), at(&["Alpha"]));

        let mut completer = FolderCompleter;
        let mut complete = |input: &str| {
            completer
                .get_completion(&format!("{parent}{input}"), None)
                .unwrap()
        };
        // The longest common part, or nothing when that adds nothing.
        assert_eq!(complete("be"), Some(format!("{parent}bet")));
        assert_eq!(complete("bet"), None);
        assert_eq!(complete("beta"), Some(at(&["beta"])[0].clone()));
        assert_eq!(complete("x"), None);

        let highlighted = at(&["alps"])[0].clone();
        assert_eq!(
            FolderCompleter
                .get_completion(&parent, Some(highlighted.clone()))
                .unwrap(),
            Some(highlighted)
        );
    }
}
