//! `orbit repo`: member Repository membership for a Board.
//!
//! Children are discovered under the Board's folder; `add` records an
//! explicit member elsewhere and points that repository's link file at this
//! Board so discovery resolves here. `remove` drops explicit membership and
//! clears the pointer when it targets this Board. `refresh` rebuilds the
//! cached Repository inventory under `.orbit/cache`.

use std::path::Path;

use serde_json::{Value, json};

use crate::board::{self, Board};
use crate::cli::{RepoAddArgs, RepoArgs, RepoCommand, RepoRemoveArgs};
use crate::error::{OrbitError, Result, code};
use crate::git;
use crate::output::CommandOutput;
use crate::repository::{self, RepositoryConfig, RepositoryKind};
use crate::task;

pub(super) fn run(board: &Board, args: RepoArgs) -> Result<CommandOutput> {
    match args.command {
        RepoCommand::Add(args) => cmd_add(board, args),
        RepoCommand::Remove(args) => cmd_remove(board, args),
        RepoCommand::List => cmd_list(board),
        RepoCommand::Refresh => cmd_refresh(board),
    }
}

fn cmd_add(board: &Board, args: RepoAddArgs) -> Result<CommandOutput> {
    let path = match args.path {
        Some(path) => board::resolve_external(&path)?,
        None => std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?,
    };
    let Some(main) = repository::main_checkout_of(&path)? else {
        return Err(OrbitError::new(
            code::NOT_A_REPOSITORY,
            format!("{} is not inside a Git repository", path.display()),
        ));
    };
    if main == repository::canonical_lenient(&board.root) {
        return Err(OrbitError::invalid_argument(
            "the Board's own repository cannot join as a member Repository",
        ));
    }
    let name = args.name.clone().unwrap_or_else(|| repository_name(&main));
    if name.trim().is_empty() || !task::is_valid_repo(&name) {
        return Err(OrbitError::invalid_argument(format!(
            "`{name}` is not a valid Repository name"
        )));
    }
    if board
        .config
        .repositories
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case(&name))
    {
        return Err(OrbitError::invalid_argument(format!(
            "a member Repository named `{name}` already exists"
        )));
    }

    let existing = repository::members(board)?
        .into_iter()
        .find(|member| member.path == main);
    let kind = match &existing {
        Some(member) if member.kind == RepositoryKind::Child => "child",
        Some(_) => "explicit",
        None => "explicit",
    };
    let mut warnings = Vec::new();
    let mut refreshed = Vec::new();
    if existing.is_none() {
        // Point the repository at this Board. Warn when the repository
        // anchors its own Board or links elsewhere: the pointer wins
        // discovery, so those Boards become shadowed.
        if board::resolve_orbit_dir(&main.join(board::ORBIT_DIR)).is_some() {
            warnings.push(format!(
                "{} anchors its own Board; the link pointer makes this Board win discovery there",
                main.display()
            ));
        }
        let toplevel = git::toplevel_containing(&main)
            .ok_or_else(|| OrbitError::new(code::NOT_A_REPOSITORY, "repository vanished"))?;
        let common = git::common_dir_at(&toplevel)?;
        let link = common.join("orbit").join("link");
        if let Ok(Some(target)) = board::read_link_file(&link)
            && let Ok((linked, _)) = board::classify_link_target(&target)
            && repository::canonical_lenient(&linked) != repository::canonical_lenient(&board.root)
        {
            warnings.push(format!(
                "replaced the repository's link to {}",
                linked.display()
            ));
        }
        board::Board::write_link_at(&common, &board.root)?;
        board.save_config("orbit: add repository", |config| {
            config.repositories.push(RepositoryConfig {
                name: name.clone(),
                path: main.clone(),
            });
            Ok(())
        })?;
        // Point configured agent MCP entries at the member's resolved Board,
        // the way `orbit link` used to refresh them.
        match crate::setup::refresh(&main) {
            Ok(outcome) => {
                refreshed = outcome
                    .files
                    .iter()
                    .filter(|file| {
                        matches!(
                            file.action,
                            crate::setup::Action::Created
                                | crate::setup::Action::Updated
                                | crate::setup::Action::Removed
                        )
                    })
                    .map(|file| file.relative.to_string_lossy().into_owned())
                    .collect();
            }
            Err(error) => warnings.push(format!(
                "could not refresh agent configuration: {error}; run `orbit setup <client>` to update MCP entries"
            )),
        }
    }

    let data = json!({
        "repository": { "name": name, "path": main, "kind": kind },
        "refreshed": refreshed,
        "board": board.root,
    });
    let human = match existing {
        Some(_) => format!(
            "{name}: already a member Repository ({kind}) at {}",
            main.display()
        ),
        None => format!("{name}: added member Repository at {}", main.display()),
    };
    let mut output = CommandOutput::new(data, human);
    for warning in warnings {
        output = output.with_warning(warning);
    }
    Ok(output)
}

fn cmd_remove(board: &Board, args: RepoRemoveArgs) -> Result<CommandOutput> {
    let key = args.repository.trim();
    if key.is_empty() {
        return Err(OrbitError::invalid_argument(
            "a member Repository name or path is required",
        ));
    }

    // Resolve a path argument to its main checkout for matching.
    let key_path = if Path::new(key).is_absolute() || key.contains(std::path::MAIN_SEPARATOR) {
        repository::main_checkout_of(Path::new(key))?
    } else {
        None
    };

    let config = board::Config::load(&board.root)?;
    let position = config
        .repositories
        .iter()
        .position(|entry| entry.name == key)
        .or_else(|| {
            key_path.as_ref().and_then(|main| {
                config
                    .repositories
                    .iter()
                    .position(|entry| repository::canonical_lenient(&entry.path) == *main)
            })
        });

    let Some(position) = position else {
        // A discovered child cannot be removed: membership follows the folder.
        if let Some(member) = repository::members(board)?
            .into_iter()
            .find(|member| member.name == key || Some(&member.path) == key_path.as_ref())
        {
            return Err(repository::child_not_removable(&member.name));
        }
        return Err(OrbitError::new(
            code::NOT_FOUND,
            format!("no member Repository named `{key}`"),
        ));
    };

    let removed = config.repositories[position].clone();
    let removed_name = removed.name.clone();
    board.save_config("orbit: remove repository", move |config| {
        if let Some(index) = config
            .repositories
            .iter()
            .position(|entry| entry.name == removed_name)
        {
            config.repositories.remove(index);
        }
        Ok(())
    })?;

    // Clear the link pointer when it still targets this Board.
    let mut link_cleared = false;
    let mut refresh_warning = None;
    if let Some(toplevel) = git::toplevel_containing(&removed.path)
        && let Ok(common) = git::common_dir_at(&toplevel)
        && let Ok(Some(target)) = board::read_link_file(&common.join("orbit").join("link"))
        && let Ok((linked, _)) = board::classify_link_target(&target)
        && repository::canonical_lenient(&linked) == repository::canonical_lenient(&board.root)
    {
        board::Board::clear_link_at(&common)?;
        link_cleared = true;
        // The repository resolves its own Board again; refresh agent entries
        // the way `orbit unlink` used to.
        if let Err(error) = crate::setup::refresh(&removed.path) {
            refresh_warning = Some(format!(
                "could not refresh agent configuration: {error}; run `orbit setup <client>` to update MCP entries"
            ));
        }
    }

    let data = json!({
        "repository": { "name": removed.name, "path": removed.path },
        "link_cleared": link_cleared,
        "board": board.root,
    });
    let mut output = CommandOutput::new(
        data,
        format!("removed member Repository `{}`", removed.name),
    );
    if let Some(warning) = refresh_warning {
        output = output.with_warning(warning);
    }
    Ok(output)
}

fn cmd_list(board: &Board) -> Result<CommandOutput> {
    let repositories: Vec<Value> = repository::members(board)?
        .iter()
        .map(|member| {
            json!({
                "name": member.name,
                "path": member.path,
                "kind": member.kind.as_str(),
            })
        })
        .collect();
    let data = json!({ "repositories": repositories, "board": board.root });
    let human = if repositories.is_empty() {
        "no member Repositories".to_string()
    } else {
        repositories
            .iter()
            .map(|entry| {
                format!(
                    "{}  {}  {}",
                    entry["name"].as_str().unwrap_or(""),
                    entry["kind"].as_str().unwrap_or(""),
                    entry["path"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(CommandOutput::new(data, human))
}

fn cmd_refresh(board: &Board) -> Result<CommandOutput> {
    let members = repository::members(board)?;
    let previous = repository::load_inventory(board)?;
    let previous_names: std::collections::BTreeSet<String> = previous
        .as_ref()
        .map(|inventory| {
            inventory
                .repositories
                .iter()
                .map(|record| record.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let current_names: std::collections::BTreeSet<String> =
        members.iter().map(|member| member.name.clone()).collect();

    let records: Vec<repository::RepositoryRecord> = members
        .iter()
        .map(|member| repository::RepositoryRecord {
            name: member.name.clone(),
            path: member.path.clone(),
            kind: member.kind,
            origin: repository::origin_of(&member.path),
        })
        .collect();
    let inventory = repository::RepositoryInventory {
        refreshed_at: chrono::Utc::now(),
        repositories: records.clone(),
    };
    repository::save_inventory(board, &inventory)?;

    let added: Vec<String> = current_names.difference(&previous_names).cloned().collect();
    let removed: Vec<String> = previous_names.difference(&current_names).cloned().collect();

    let data = json!({
        "repositories": serde_json::to_value(&records).unwrap_or(Value::Null),
        "added": added,
        "removed": removed,
        "inventory": repository::inventory_path(board),
        "board": board.root,
    });
    let human = format!(
        "refreshed {} member Repositories ({} added, {} removed)",
        records.len(),
        data["added"].as_array().map_or(0, Vec::len),
        data["removed"].as_array().map_or(0, Vec::len),
    );
    Ok(CommandOutput::new(data, human))
}

fn repository_name(main: &Path) -> String {
    main.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".to_string())
}
