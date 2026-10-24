use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{OrbitError, Result};
use crate::output::CommandOutput;

pub const BEGIN_MARKER: &str = "<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->";
pub const END_MARKER: &str = "<!-- END ORBIT INTEGRATION -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupClient {
    Claude,
    Cursor,
    Codex,
    Opencode,
    Mattpocock,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Created,
    Updated,
    Removed,
    Unchanged,
    Skipped,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Created => "created",
            Action::Updated => "updated",
            Action::Removed => "removed",
            Action::Unchanged => "unchanged",
            Action::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum McpJsonKind {
    Servers,
    Opencode,
}

impl McpJsonKind {
    fn container(self) -> &'static str {
        match self {
            McpJsonKind::Servers => "mcpServers",
            McpJsonKind::Opencode => "mcp",
        }
    }

    fn schema(self) -> Option<&'static str> {
        match self {
            McpJsonKind::Servers => None,
            McpJsonKind::Opencode => Some("https://opencode.ai/config.json"),
        }
    }
}

#[derive(Debug, Clone)]
struct FilePlan {
    absolute: PathBuf,
    relative: PathBuf,
    action: Action,
    content: Option<String>,
    warning: Option<String>,
}

pub fn run(args: crate::cli::SetupArgs, dir: Option<&Path>) -> Result<CommandOutput> {
    let root = project_root()?;
    let mcp_dir = dir.map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut plan = BTreeMap::new();
    let mut warnings = Vec::new();

    if matches!(args.client, SetupClient::Mattpocock) && (args.mcp || args.mcp_bin.is_some()) {
        warnings.push("mattpocock is not an MCP client; ignoring MCP options".to_string());
    }
    if args.hooks && !args.remove && !matches!(args.client, SetupClient::Claude | SetupClient::All)
    {
        warnings.push("hooks are only supported for Claude in M1; ignoring --hooks".to_string());
    }

    let mcp = !args.no_mcp;
    let mcp_bin = if args.remove || !mcp {
        None
    } else {
        Some(resolve_mcp_bin(args.mcp_bin.as_deref())?)
    };

    if let Some(bin) = &mcp_bin {
        if bin.components().count() == 1 {
            warnings.push(
                "MCP config uses the bare `orbit` command; the editor's PATH must contain Orbit"
                    .to_string(),
            );
        } else if is_build_artifact(bin) {
            warnings.push(format!(
                "MCP config points at a build artifact ({}); install Orbit to a stable location or re-run setup after rebuilding",
                bin.display()
            ));
        } else if bin.starts_with(std::env::temp_dir()) {
            warnings.push(format!(
                "MCP config points at a temporary path ({}); install Orbit to a stable location",
                bin.display()
            ));
        }
    }

    for client in expand(args.client) {
        if args.remove {
            plan_remove(&root, client, &mut plan, &mut warnings, mcp_dir.as_deref())?;
        } else {
            plan_install(
                &root,
                client,
                args.hooks,
                mcp,
                mcp_bin.as_deref(),
                mcp_dir.as_deref(),
                &mut plan,
                &mut warnings,
            )?;
        }
    }

    let files = if args.print {
        plan.into_values().collect::<Vec<_>>()
    } else {
        apply_plan(plan)?
    };

    let file_values: Vec<Value> = files
        .iter()
        .map(|file| {
            let mut value = json!({
                "path": file.relative.to_string_lossy(),
                "action": file.action,
            });
            if args.print
                && let Some(content) = &file.content
            {
                value["content"] = json!(content);
            }
            value
        })
        .collect();

    for file in &files {
        if let Some(warning) = &file.warning {
            warnings.push(warning.clone());
        }
    }

    let human = render_human(&root, args.print, &files);
    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "preview": args.print,
            "files": file_values,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

fn expand(client: SetupClient) -> Vec<SetupClient> {
    match client {
        SetupClient::All => vec![
            SetupClient::Claude,
            SetupClient::Cursor,
            SetupClient::Codex,
            SetupClient::Opencode,
            SetupClient::Mattpocock,
        ],
        other => vec![other],
    }
}

fn project_root() -> Result<PathBuf> {
    match crate::git::repo_root() {
        Ok(root) => Ok(root),
        Err(_) => std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error)),
    }
}

fn plan_install(
    root: &Path,
    client: SetupClient,
    hooks: bool,
    mcp: bool,
    mcp_bin: Option<&Path>,
    mcp_dir: Option<&Path>,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mcp_bin = mcp_bin.filter(|_| mcp);
    match client {
        SetupClient::Claude => {
            plan_section(root, "CLAUDE.md", &workflow_section(), plan)?;
            if hooks {
                plan_claude_hooks(root, false, plan)?;
            }
            if mcp {
                plan_mcp_json(
                    root,
                    ".mcp.json",
                    McpJsonKind::Servers,
                    mcp_servers_entry(
                        mcp_bin.expect("MCP binary resolved when MCP is enabled"),
                        mcp_dir,
                    ),
                    false,
                    plan,
                    warnings,
                )?;
            }
        }
        SetupClient::Cursor => {
            plan_full_file(root, ".cursor/rules/orbit.mdc", &cursor_rule(), plan)?;
            plan_full_file(root, ".agents/skills/orbit/SKILL.md", &skill_file(), plan)?;
            if mcp {
                plan_mcp_json(
                    root,
                    ".cursor/mcp.json",
                    McpJsonKind::Servers,
                    mcp_servers_entry(
                        mcp_bin.expect("MCP binary resolved when MCP is enabled"),
                        mcp_dir,
                    ),
                    false,
                    plan,
                    warnings,
                )?;
            }
        }
        SetupClient::Codex => {
            plan_full_file(root, ".agents/skills/orbit/SKILL.md", &skill_file(), plan)?;
            plan_section(root, "AGENTS.md", &workflow_section(), plan)?;
            if mcp {
                plan_mcp_toml(
                    root,
                    ".codex/config.toml",
                    mcp_args(mcp_dir),
                    false,
                    plan,
                    warnings,
                )?;
            }
        }
        SetupClient::Opencode => {
            plan_section(root, "AGENTS.md", &workflow_section(), plan)?;
            if mcp {
                plan_mcp_json(
                    root,
                    "opencode.json",
                    McpJsonKind::Opencode,
                    opencode_entry(mcp_dir),
                    false,
                    plan,
                    warnings,
                )?;
            }
        }
        SetupClient::Mattpocock => {
            plan_section(
                root,
                "docs/agents/issue-tracker.md",
                &mattpocock_section(),
                plan,
            )?;
        }
        SetupClient::All => unreachable!("All is expanded before planning"),
    }
    Ok(())
}

fn plan_remove(
    root: &Path,
    client: SetupClient,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
    mcp_dir: Option<&Path>,
) -> Result<()> {
    match client {
        SetupClient::Claude => {
            plan_section_remove(root, "CLAUDE.md", plan)?;
            plan_claude_hooks(root, true, plan)?;
            plan_mcp_json(
                root,
                ".mcp.json",
                McpJsonKind::Servers,
                mcp_servers_entry(mcp_dir),
                true,
                plan,
                warnings,
            )?;
        }
        SetupClient::Cursor => {
            plan_full_file_remove(root, ".cursor/rules/orbit.mdc", plan)?;
            plan_full_file_remove(root, ".agents/skills/orbit/SKILL.md", plan)?;
            plan_mcp_json(
                root,
                ".cursor/mcp.json",
                McpJsonKind::Servers,
                mcp_servers_entry(mcp_dir),
                true,
                plan,
                warnings,
            )?;
        }
        SetupClient::Codex => {
            plan_full_file_remove(root, ".agents/skills/orbit/SKILL.md", plan)?;
            plan_section_remove(root, "AGENTS.md", plan)?;
            plan_mcp_toml(
                root,
                ".codex/config.toml",
                mcp_args(mcp_dir),
                true,
                plan,
                warnings,
            )?;
        }
        SetupClient::Opencode => {
            plan_section_remove(root, "AGENTS.md", plan)?;
            plan_mcp_json(
                root,
                "opencode.json",
                McpJsonKind::Opencode,
                opencode_entry(mcp_dir),
                true,
                plan,
                warnings,
            )?;
        }
        SetupClient::Mattpocock => {
            plan_section_remove(root, "docs/agents/issue-tracker.md", plan)?;
        }
        SetupClient::All => unreachable!("All is expanded before planning"),
    }
    Ok(())
}

fn plan_section(
    root: &Path,
    relative: &str,
    section: &str,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let content = upsert_section(existing.as_deref().unwrap_or(""), section, relative)?;
    let action = match &existing {
        None => Action::Created,
        Some(existing) if existing == &content => Action::Unchanged,
        Some(_) => Action::Updated,
    };
    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content: Some(content),
            warning: None,
        },
    )
}

fn plan_section_remove(
    root: &Path,
    relative: &str,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let Some(existing) = read_optional(&absolute)? else {
        return Ok(());
    };
    let (content, changed) = remove_section(&existing, relative)?;
    if !changed {
        return Ok(());
    }
    let (action, content) = if content.trim().is_empty() {
        (Action::Removed, None)
    } else {
        (Action::Updated, Some(content))
    };
    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content,
            warning: None,
        },
    )
}

fn plan_full_file(
    root: &Path,
    relative: &str,
    content: &str,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let action = match &existing {
        None => Action::Created,
        Some(existing) if !existing.contains(BEGIN_MARKER) => {
            return Err(OrbitError::config(format!(
                "{relative} exists and is not managed by Orbit; move it or use --print"
            )));
        }
        Some(existing) if existing == content => Action::Unchanged,
        Some(_) => Action::Updated,
    };
    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content: Some(content.to_string()),
            warning: None,
        },
    )
}

fn plan_full_file_remove(
    root: &Path,
    relative: &str,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let Some(existing) = read_optional(&absolute)? else {
        return Ok(());
    };
    if existing.contains(BEGIN_MARKER) {
        insert_plan(
            plan,
            FilePlan {
                absolute,
                relative: PathBuf::from(relative),
                action: Action::Removed,
                content: None,
                warning: None,
            },
        )?;
    } else {
        let warning = format!("{relative} is not managed by Orbit; leaving it in place");
        insert_plan(
            plan,
            FilePlan {
                absolute,
                relative: PathBuf::from(relative),
                action: Action::Skipped,
                content: None,
                warning: Some(warning),
            },
        )?;
    }
    Ok(())
}

fn plan_claude_hooks(
    root: &Path,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let relative = ".claude/settings.json";
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let mut value: Value = match &existing {
        Some(content) => serde_json::from_str(content).map_err(|error| {
            OrbitError::config(format!("invalid .claude/settings.json: {error}"))
        })?,
        None => json!({}),
    };

    let changed = if remove {
        remove_orbit_hook(&mut value)?
    } else {
        add_orbit_hook(&mut value)?
    };
    if !changed {
        return Ok(());
    }

    let content = format!(
        "{}\n",
        serde_json::to_string_pretty(&value)
            .map_err(|error| OrbitError::config(format!("serialize settings.json: {error}")))?
    );
    let empty = value.as_object().is_some_and(|object| object.is_empty());
    let action = if existing.is_none() {
        Action::Created
    } else if remove && empty {
        Action::Removed
    } else {
        Action::Updated
    };
    let content = if action == Action::Removed {
        None
    } else {
        Some(content)
    };
    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content,
            warning: None,
        },
    )
}

fn mcp_args(dir: Option<&Path>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(dir) = dir {
        args.push("--dir".to_string());
        args.push(dir.to_string_lossy().to_string());
    }
    args.push("mcp".to_string());
    args
}

fn mcp_servers_entry(dir: Option<&Path>) -> Value {
    json!({
        "command": "orbit",
        "args": mcp_args(dir)
    })
}

fn opencode_entry(dir: Option<&Path>) -> Value {
    let mut command = vec!["orbit".to_string()];
    command.extend(mcp_args(dir));
    json!({
        "type": "local",
        "command": command,
        "enabled": true
    })
}

fn plan_mcp_json(
    root: &Path,
    relative: &str,
    kind: McpJsonKind,
    entry: Value,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let container = kind.container();
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let mut value: Value = match &existing {
        Some(content) => serde_json::from_str(content)
            .map_err(|error| OrbitError::config(format!("invalid {relative}: {error}")))?,
        None => json!({}),
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| OrbitError::config(format!("{relative} must contain a JSON object")))?;

    let mut changed = false;
    let mut warning = None;

    if remove {
        if let Some(container_value) = object.get_mut(container).and_then(Value::as_object_mut) {
            if let Some(existing_entry) = container_value.get("orbit") {
                if is_orbit_mcp_entry(existing_entry) {
                    container_value.remove("orbit");
                    changed = true;
                } else {
                    warning = Some(format!(
                        "{relative} has an unrelated `orbit` server; leaving it"
                    ));
                }
            }
            if container_value.is_empty() {
                object.remove(container);
            }
        }
    } else {
        if existing.is_none()
            && let Some(schema) = kind.schema()
        {
            object.insert("$schema".to_string(), json!(schema));
        }
        let container_value = object
            .entry(container)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                OrbitError::config(format!("{relative} `{container}` must be an object"))
            })?;

        let existing_entry = container_value.get("orbit").cloned();
        match existing_entry {
            Some(existing_entry) if existing_entry == entry => {}
            Some(existing_entry) if is_orbit_mcp_entry(&existing_entry) => {
                container_value.insert("orbit".to_string(), entry);
                changed = true;
            }
            Some(_) => {
                return Err(OrbitError::config(format!(
                    "{relative} already has an `orbit` server that Orbit does not manage; move it first"
                )));
            }
            None => {
                container_value.insert("orbit".to_string(), entry);
                changed = true;
            }
        }
    }

    if let Some(warning) = warning {
        warnings.push(warning);
    }
    if !changed {
        return Ok(());
    }

    let empty = value.as_object().is_some_and(|object| {
        object.is_empty() || (object.len() == 1 && object.contains_key("$schema"))
    });
    let action = if existing.is_none() {
        Action::Created
    } else if empty {
        Action::Removed
    } else {
        Action::Updated
    };
    let content = if action == Action::Removed {
        None
    } else {
        Some(format!(
            "{}\n",
            serde_json::to_string_pretty(&value).map_err(|error| {
                OrbitError::config(format!("serialize {relative}: {error}"))
            })?
        ))
    };

    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content,
            warning: None,
        },
    )
}

fn plan_mcp_toml(
    root: &Path,
    relative: &str,
    args: Vec<String>,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let mut value: toml::Value = match &existing {
        Some(content) => toml::from_str(content)
            .map_err(|error| OrbitError::config(format!("invalid {relative}: {error}")))?,
        None => toml::Value::Table(toml::Table::new()),
    };
    let table = value
        .as_table_mut()
        .ok_or_else(|| OrbitError::config(format!("{relative} must contain a TOML table")))?;

    let mut changed = false;
    let mut warning = None;

    if remove {
        if let Some(servers) = table
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
        {
            if let Some(existing_entry) = servers.get("orbit") {
                if is_orbit_toml_entry(existing_entry) {
                    servers.remove("orbit");
                    changed = true;
                } else {
                    warning = Some(format!(
                        "{relative} has an unrelated `orbit` server; leaving it"
                    ));
                }
            }
            if servers.is_empty() {
                table.remove("mcp_servers");
            }
        }
    } else {
        if !table.contains_key("mcp_servers") {
            table.insert(
                "mcp_servers".to_string(),
                toml::Value::Table(toml::Table::new()),
            );
        }
        let servers = table
            .get_mut("mcp_servers")
            .and_then(toml::Value::as_table_mut)
            .ok_or_else(|| {
                OrbitError::config(format!("{relative} `mcp_servers` must be a table"))
            })?;
        let expected = toml_mcp_entry(&args);
        let existing_entry = servers.get("orbit").cloned();
        match existing_entry {
            Some(existing_entry) if existing_entry == expected => {}
            Some(existing_entry) if is_orbit_toml_entry(&existing_entry) => {
                servers.insert("orbit".to_string(), expected);
                changed = true;
            }
            Some(_) => {
                return Err(OrbitError::config(format!(
                    "{relative} already has an `orbit` server that Orbit does not manage; move it first"
                )));
            }
            None => {
                servers.insert("orbit".to_string(), expected);
                changed = true;
            }
        }
    }

    if let Some(warning) = warning {
        warnings.push(warning);
    }
    if !changed {
        return Ok(());
    }

    let empty = value.as_table().is_some_and(|table| table.is_empty());
    let action = if existing.is_none() {
        Action::Created
    } else if empty {
        Action::Removed
    } else {
        Action::Updated
    };
    let content = if action == Action::Removed {
        None
    } else {
        Some(
            toml::to_string_pretty(&value)
                .map_err(|error| OrbitError::config(format!("serialize {relative}: {error}")))?,
        )
    };

    insert_plan(
        plan,
        FilePlan {
            absolute,
            relative: PathBuf::from(relative),
            action,
            content,
            warning: None,
        },
    )
}

fn toml_mcp_entry(args: &[String]) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "command".to_string(),
        toml::Value::String("orbit".to_string()),
    );
    table.insert(
        "args".to_string(),
        toml::Value::Array(args.iter().cloned().map(toml::Value::String).collect()),
    );
    toml::Value::Table(table)
}

fn is_orbit_mcp_entry(entry: &Value) -> bool {
    let command_is_orbit = entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            command == "orbit" || command.ends_with("orbit") || command.ends_with("orbit.exe")
        })
        || entry
            .get("command")
            .and_then(Value::as_array)
            .and_then(|command| command.first())
            .and_then(Value::as_str)
            .is_some_and(|command| {
                command == "orbit" || command.ends_with("orbit") || command.ends_with("orbit.exe")
            });

    let has_mcp = entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("mcp")))
        || entry
            .get("command")
            .and_then(Value::as_array)
            .is_some_and(|command| command.iter().any(|arg| arg.as_str() == Some("mcp")));

    command_is_orbit && has_mcp
}

fn is_orbit_toml_entry(entry: &toml::Value) -> bool {
    let command_is_orbit = entry
        .get("command")
        .and_then(toml::Value::as_str)
        .is_some_and(|command| {
            command == "orbit" || command.ends_with("orbit") || command.ends_with("orbit.exe")
        });
    let has_mcp = entry
        .get("args")
        .and_then(toml::Value::as_array)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("mcp")));
    command_is_orbit && has_mcp
}

fn insert_plan(plan: &mut BTreeMap<PathBuf, FilePlan>, file: FilePlan) -> Result<()> {
    if let Some(existing) = plan.get(&file.relative) {
        if existing.action != file.action || existing.content != file.content {
            return Err(OrbitError::config(format!(
                "setup produced conflicting plans for {}",
                file.relative.display()
            )));
        }
    } else {
        plan.insert(file.relative.clone(), file);
    }
    Ok(())
}

fn apply_plan(plan: BTreeMap<PathBuf, FilePlan>) -> Result<Vec<FilePlan>> {
    let mut results = Vec::new();
    for file in plan.into_values() {
        match file.action {
            Action::Created | Action::Updated => {
                if let Some(content) = &file.content {
                    if let Some(parent) = file.absolute.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| {
                            OrbitError::io(format!("create {}", parent.display()), error)
                        })?;
                    }
                    crate::atomic::write_atomic(&file.absolute, content.as_bytes(), true)?;
                }
            }
            Action::Removed => match std::fs::remove_file(&file.absolute) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(OrbitError::io(
                        format!("remove {}", file.absolute.display()),
                        error,
                    ));
                }
            },
            Action::Unchanged | Action::Skipped => {}
        }
        results.push(file);
    }
    Ok(results)
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(OrbitError::io(format!("read {}", path.display()), error)),
    }
}

fn upsert_section(existing: &str, section: &str, relative: &str) -> Result<String> {
    let begin = existing.find(BEGIN_MARKER);
    let end = existing.find(END_MARKER);
    match (begin, end) {
        (Some(begin), Some(end)) if end > begin => {
            let end = end + END_MARKER.len();
            let before = existing[..begin].trim_end_matches('\n');
            let after = existing[end..].trim_start_matches('\n');
            let mut result = String::new();
            if !before.is_empty() {
                result.push_str(before);
                result.push_str("\n\n");
            }
            result.push_str(section);
            if !after.is_empty() {
                result.push_str("\n\n");
                result.push_str(after);
            }
            result.push('\n');
            Ok(result)
        }
        (None, None) => {
            let trimmed = existing.trim_end_matches('\n');
            if trimmed.is_empty() {
                Ok(format!("{section}\n"))
            } else {
                Ok(format!("{trimmed}\n\n{section}\n"))
            }
        }
        _ => Err(OrbitError::config(format!(
            "{relative} has an incomplete Orbit managed section"
        ))),
    }
}

fn remove_section(existing: &str, relative: &str) -> Result<(String, bool)> {
    let begin = existing.find(BEGIN_MARKER);
    let end = existing.find(END_MARKER);
    match (begin, end) {
        (Some(begin), Some(end)) if end > begin => {
            let end = end + END_MARKER.len();
            let before = existing[..begin].trim_end_matches('\n');
            let after = existing[end..].trim_start_matches('\n');
            let mut result = String::new();
            if !before.is_empty() {
                result.push_str(before);
                result.push('\n');
            }
            if !after.is_empty() {
                result.push_str(after);
                result.push('\n');
            }
            Ok((result, true))
        }
        (None, None) => Ok((existing.to_string(), false)),
        _ => Err(OrbitError::config(format!(
            "{relative} has an incomplete Orbit managed section"
        ))),
    }
}

fn add_orbit_hook(value: &mut Value) -> Result<bool> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| OrbitError::config(".claude/settings.json must contain a JSON object"))?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| OrbitError::config(".claude/settings.json hooks must be an object"))?;
    let session_start = hooks
        .entry("SessionStart")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| {
            OrbitError::config(".claude/settings.json hooks.SessionStart must be an array")
        })?;

    let already_present = session_start.iter().any(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|handlers| handlers.iter().any(is_orbit_hook))
    });
    if already_present {
        return Ok(false);
    }

    session_start.push(json!({
        "matcher": "",
        "hooks": [
            { "type": "command", "command": "orbit prime --hook-json --by claude" }
        ]
    }));
    Ok(true)
}

fn remove_orbit_hook(value: &mut Value) -> Result<bool> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| OrbitError::config(".claude/settings.json must contain a JSON object"))?;
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let Some(session_start) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) else {
        return Ok(false);
    };

    let mut changed = false;
    session_start.retain_mut(|group| {
        let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = handlers.len();
        handlers.retain(|handler| !is_orbit_hook(handler));
        if handlers.len() != before {
            changed = true;
        }
        !handlers.is_empty()
    });

    if session_start.is_empty() {
        hooks.remove("SessionStart");
    }
    if hooks.is_empty() {
        object.remove("hooks");
    }
    Ok(changed)
}

fn is_orbit_hook(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains("orbit prime"))
}

fn workflow_section() -> String {
    format!(
        "{BEGIN_MARKER}
## Orbit task tracking

Orbit is the task tracker for this repository. Use the `orbit` CLI; never edit task files or YAML frontmatter directly.

- `orbit ready --claim --by <agent>` claims the highest-priority ready task atomically.
- `orbit heartbeat <id> --by <agent>` extends the lease while you work.
- `orbit done <id> --by <agent>` completes it; `orbit cancel <id> --by <agent>` abandons it.
- `orbit add \"title\" -p N --depends-on <id>` creates work.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- Run `orbit prime` at session start for ready work, stale claims, and counts.

Parse the JSON envelope: `ok`, `data`, `error.code`, and `error.retryable`. If a command returns `LOCKED`, retry after a short backoff. Never infer state from prose.
{END_MARKER}"
    )
}

fn cursor_rule() -> String {
    format!(
        "---
description: \"Orbit task tracker workflow\"
globs: [\"**/*\"]
alwaysApply: true
---

{}
",
        workflow_section()
    )
}

fn skill_file() -> String {
    format!(
        "---
name: orbit
description: \"Use the Orbit CLI to claim, track, and complete tasks in this repository. Trigger when the user asks what to work on, asks to create or update a task, or starts a multi-step implementation.\"
---

{}
",
        workflow_section()
    )
}

fn mattpocock_section() -> String {
    format!(
        "{BEGIN_MARKER}
## Orbit

We use Orbit, a local AI-native issue tracker. Tasks live as Markdown on the board and are served by the `orbit` CLI; the board is shared by every Git worktree.

- To find actionable work, run `orbit ready --json` and parse `data.tasks`.
- To take work, run `orbit ready --claim --by <agent>` or `orbit claim <ID> --by <agent>`.
- To finish or abandon work, run `orbit done <ID> --by <agent>` or `orbit cancel <ID> --by <agent>`.
- To create work, run `orbit add \"Title\" -p N --depends-on <ID>`.
- To label work, use `orbit label add <ID> <label>...` and `orbit label remove <ID> <label>...`; filter with `orbit list --label <label>` or `orbit list --unlabeled`.
- To comment, use `orbit comment add <ID> --body \"...\" --by <agent>` and `orbit comment list <ID>`.
- The hold labels `needs-triage`, `needs-info`, and `ready-for-human` keep tasks out of the ready queue and prevent claims until removed. `ready-for-agent`, `bug`, and `enhancement` are neutral.
- Never edit task frontmatter directly; that bypasses dependency validation, leases, and the write lock.
- Use `orbit show <ID>` to read acceptance criteria.
{END_MARKER}"
    )
}

fn render_human(root: &Path, preview: bool, files: &[FilePlan]) -> String {
    let mut output = String::new();
    if preview {
        output.push_str(&format!("setup preview (root: {})\n", root.display()));
    }
    if files.is_empty() {
        output.push_str("no changes\n");
        return output;
    }

    for file in files {
        output.push_str(&format!(
            "{} {}\n",
            file.action.as_str(),
            file.relative.display()
        ));
        if preview && let Some(content) = &file.content {
            output.push_str(&format!("--- {} ---\n{content}", file.relative.display()));
            if !content.ends_with('\n') {
                output.push('\n');
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_remove_section_round_trip() {
        let existing = "# Project\n\nExisting instructions.\n";
        let section = workflow_section();
        let updated = upsert_section(existing, &section, "CLAUDE.md").unwrap();
        assert!(updated.contains("Existing instructions."));
        assert!(updated.contains(BEGIN_MARKER));
        assert!(updated.contains(END_MARKER));

        let (restored, changed) = remove_section(&updated, "CLAUDE.md").unwrap();
        assert!(changed);
        assert!(!restored.contains(BEGIN_MARKER));
        assert!(restored.contains("Existing instructions."));
    }

    #[test]
    fn upsert_is_idempotent() {
        let section = workflow_section();
        let first = upsert_section("", &section, "AGENTS.md").unwrap();
        let second = upsert_section(&first, &section, "AGENTS.md").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.matches(BEGIN_MARKER).count(), 1);
    }

    #[test]
    fn incomplete_markers_are_rejected() {
        let error = upsert_section(BEGIN_MARKER, "section", "CLAUDE.md").unwrap_err();
        assert_eq!(error.code, crate::error::code::CONFIG_ERROR);
    }

    #[test]
    fn hook_is_added_once_and_removed_without_touching_other_settings() {
        let mut value = json!({
            "permissions": { "allow": ["Bash(git status)"] }
        });
        assert!(add_orbit_hook(&mut value).unwrap());
        assert!(!add_orbit_hook(&mut value).unwrap());
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "orbit prime --hook-json --by claude"
        );
        assert_eq!(value["permissions"]["allow"][0], "Bash(git status)");

        assert!(remove_orbit_hook(&mut value).unwrap());
        assert!(value.get("hooks").is_none());
        assert_eq!(value["permissions"]["allow"][0], "Bash(git status)");
    }
}
