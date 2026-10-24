use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::ValueEnum;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::{OrbitError, Result, code};
use crate::output::CommandOutput;

pub const BEGIN_MARKER: &str = "<!-- BEGIN ORBIT INTEGRATION (managed by orbit setup) -->";
pub const END_MARKER: &str = "<!-- END ORBIT INTEGRATION -->";

/// Completion guidance shared by `orbit prime` and every generated agent file.
pub const COMPLETION_LINE: &str = "- `orbit done <id> --by <agent> --comment \"...\"` completes it and records the note with the status change; `orbit cancel <id> --by <agent>` abandons it.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupClient {
    Claude,
    Cursor,
    Codex,
    Opencode,
    Mattpocock,
    All,
}

impl SetupClient {
    pub fn name(self) -> &'static str {
        match self {
            SetupClient::Claude => "claude",
            SetupClient::Cursor => "cursor",
            SetupClient::Codex => "codex",
            SetupClient::Opencode => "opencode",
            SetupClient::Mattpocock => "mattpocock",
            SetupClient::All => "all",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "claude" => Some(SetupClient::Claude),
            "cursor" => Some(SetupClient::Cursor),
            "codex" => Some(SetupClient::Codex),
            "opencode" => Some(SetupClient::Opencode),
            "mattpocock" => Some(SetupClient::Mattpocock),
            "all" => Some(SetupClient::All),
            _ => None,
        }
    }

    pub fn all() -> Vec<SetupClient> {
        vec![
            SetupClient::Claude,
            SetupClient::Cursor,
            SetupClient::Codex,
            SetupClient::Opencode,
            SetupClient::Mattpocock,
        ]
    }
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

struct McpJsonLayout {
    container_path: &'static [&'static str],
    legacy_container_path: Option<&'static [&'static str]>,
    schema: Option<&'static str>,
}

impl McpJsonKind {
    fn layout(self) -> McpJsonLayout {
        match self {
            McpJsonKind::Servers => McpJsonLayout {
                container_path: &["mcpServers"],
                legacy_container_path: None,
                schema: None,
            },
            McpJsonKind::Opencode => McpJsonLayout {
                container_path: &["mcp", "servers"],
                legacy_container_path: Some(&["mcp"]),
                schema: Some("https://opencode.ai/config.json"),
            },
        }
    }
}

fn nested_object_mut<'a>(
    object: &'a mut serde_json::Map<String, Value>,
    path: &[&str],
) -> Option<&'a mut serde_json::Map<String, Value>> {
    let (head, rest) = path.split_first()?;
    let child = object.get_mut(*head)?.as_object_mut()?;
    if rest.is_empty() {
        Some(child)
    } else {
        nested_object_mut(child, rest)
    }
}

fn ensure_nested_object_mut<'a>(
    object: &'a mut serde_json::Map<String, Value>,
    path: &[&str],
    relative: &str,
) -> Result<&'a mut serde_json::Map<String, Value>> {
    let (head, rest) = path.split_first().expect("container path is not empty");
    let child = object
        .entry((*head).to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            OrbitError::config(format!("{relative} `{}` must be an object", path.join(".")))
        })?;
    if rest.is_empty() {
        Ok(child)
    } else {
        ensure_nested_object_mut(child, rest, relative)
    }
}

fn prune_empty_object_path(object: &mut serde_json::Map<String, Value>, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    let Some(child) = object.get_mut(*head).and_then(Value::as_object_mut) else {
        return;
    };
    if !rest.is_empty() {
        prune_empty_object_path(child, rest);
    }
    if child.is_empty() {
        object.remove(*head);
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

pub struct InstallOptions {
    pub clients: Vec<SetupClient>,
    pub hooks: bool,
    pub hook_bin: Option<PathBuf>,
    pub mcp_bin: Option<PathBuf>,
    pub mcp_dir: Option<PathBuf>,
    pub print: bool,
    pub remove: bool,
}

pub fn run(args: crate::cli::SetupArgs, dir: Option<&Path>) -> Result<CommandOutput> {
    let root = project_root()?;
    let linked = crate::board::Board::read_link().ok().flatten();
    if let Some(path) = &linked
        && !crate::board::Board::is_board(path)
    {
        return Err(OrbitError::new(
            code::LINK_NOT_FOUND,
            format!(
                "the linked Board at {} is missing; run `orbit link <board>` to re-link or restore it",
                path.display()
            ),
        ));
    }
    let mcp_dir = dir
        .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
        .or_else(|| linked.map(|path| dunce::canonicalize(&path).unwrap_or(path)));
    let mut warnings = Vec::new();

    if matches!(args.client, SetupClient::Mattpocock) && (args.mcp || args.mcp_bin.is_some()) {
        warnings.push("mattpocock is not an MCP client; ignoring MCP options".to_string());
    }

    let hooks =
        args.hooks && !args.remove && matches!(args.client, SetupClient::Claude | SetupClient::All);
    if args.hooks && !args.remove && !hooks {
        warnings.push("hooks are only supported for Claude in M1; ignoring --hooks".to_string());
    }

    let mcp = !args.no_mcp;
    let needs_bin = (mcp || hooks) && !args.remove;
    let bin = if needs_bin {
        Some(resolve_mcp_bin(args.mcp_bin.as_deref())?)
    } else {
        None
    };

    if let Some(bin) = &bin
        && bin.components().count() == 1
    {
        warnings.push(
            "MCP config uses the bare `orbit` command; the editor's PATH must contain Orbit"
                .to_string(),
        );
    } else if let Some(bin) = &bin
        && is_build_artifact(bin)
    {
        warnings.push(format!(
            "agent config points at a build artifact ({}); install Orbit to a stable location or re-run setup after rebuilding",
            bin.display()
        ));
    }

    let clients = expand(args.client);

    if args.check {
        let preview = install(
            &root,
            InstallOptions {
                clients,
                hooks,
                hook_bin: bin.clone(),
                mcp_bin: if mcp { bin } else { None },
                mcp_dir,
                print: true,
                remove: false,
            },
        )?;
        let mut output = check_output(&root, preview);
        output.warnings.extend(warnings);
        return Ok(output);
    }

    let mut output = install(
        &root,
        InstallOptions {
            clients,
            hooks,
            hook_bin: bin.clone(),
            mcp_bin: if mcp { bin } else { None },
            mcp_dir,
            print: args.print,
            remove: args.remove,
        },
    )?;
    output.warnings.extend(warnings);
    Ok(output)
}

pub fn install(root: &Path, options: InstallOptions) -> Result<CommandOutput> {
    let mut plan = BTreeMap::new();
    let mut warnings = Vec::new();

    for client in options.clients {
        if options.remove {
            plan_remove(
                root,
                client,
                &mut plan,
                &mut warnings,
                options.mcp_dir.as_deref(),
            )?;
        } else {
            plan_install(
                root,
                client,
                &PlanOptions {
                    hooks: options.hooks,
                    hook_bin: options.hook_bin.as_deref(),
                    mcp_bin: options.mcp_bin.as_deref(),
                    mcp_dir: options.mcp_dir.as_deref(),
                },
                &mut plan,
                &mut warnings,
            )?;
        }
    }

    let files = if options.print {
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
            if options.print
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

    let human = render_human(root, options.print, &files);
    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "preview": options.print,
            "files": file_values,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

pub fn refresh(root: &Path, mcp_dir: Option<&Path>) -> Result<CommandOutput> {
    let clients = configured_clients(root);
    if clients.is_empty() {
        return Ok(CommandOutput::new(
            json!({
                "root": root.to_string_lossy(),
                "preview": false,
                "files": [],
            }),
            "no agent integration to refresh\n",
        ));
    }

    let bin = resolve_mcp_bin(None)?;
    install(
        root,
        InstallOptions {
            clients,
            hooks: false,
            hook_bin: None,
            mcp_bin: Some(bin),
            mcp_dir: mcp_dir.map(Path::to_path_buf),
            print: false,
            remove: false,
        },
    )
}

pub(crate) fn has_orbit_hook(root: &Path) -> bool {
    std::fs::read_to_string(root.join(".claude/settings.json"))
        .is_ok_and(|content| content.contains("orbit prime"))
}

fn configured_clients(root: &Path) -> Vec<SetupClient> {
    let contains = |relative: &str, needle: &str| {
        std::fs::read_to_string(root.join(relative)).is_ok_and(|content| content.contains(needle))
    };
    let mut clients = Vec::new();

    if contains("CLAUDE.md", BEGIN_MARKER)
        || contains(".mcp.json", "\"orbit\"")
        || contains(".claude/settings.json", "orbit prime")
    {
        clients.push(SetupClient::Claude);
    }
    if contains(".cursor/rules/orbit.mdc", BEGIN_MARKER)
        || contains(".cursor/mcp.json", "\"orbit\"")
    {
        clients.push(SetupClient::Cursor);
    }
    if contains(".codex/config.toml", "orbit")
        || contains(".agents/skills/orbit/SKILL.md", BEGIN_MARKER)
    {
        clients.push(SetupClient::Codex);
    }
    if contains("opencode.json", "\"orbit\"") {
        clients.push(SetupClient::Opencode);
    }
    if root.join("docs/agents/issue-tracker.md").is_file() {
        clients.push(SetupClient::Mattpocock);
    }

    clients
}

fn check_output(root: &Path, preview: CommandOutput) -> CommandOutput {
    let files = preview
        .data
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let status = check_status(&files);

    let mut human = format!("orbit setup check: {status}\n");
    for file in &files {
        human.push_str(&format!(
            "{} {}\n",
            file["action"].as_str().unwrap_or("unknown"),
            file["path"].as_str().unwrap_or("")
        ));
    }

    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "status": status,
            "files": files,
        }),
        human,
    );
    output.warnings = preview.warnings;
    output.exit_code = if status == "installed" { 0 } else { 1 };
    output
}

fn check_status(files: &[Value]) -> &'static str {
    let mut created = false;
    let mut updated = false;
    for file in files {
        match file["action"].as_str() {
            Some("created") => created = true,
            Some("updated") => updated = true,
            _ => {}
        }
    }
    if updated {
        "stale"
    } else if created {
        "missing"
    } else {
        "installed"
    }
}

fn expand(client: SetupClient) -> Vec<SetupClient> {
    match client {
        SetupClient::All => SetupClient::all(),
        other => vec![other],
    }
}

pub fn detect(root: &Path) -> Vec<SetupClient> {
    let home = home_dir();
    let home_has = |relative: &str| {
        home.as_ref()
            .is_some_and(|home| home.join(relative).exists())
    };

    SetupClient::all()
        .into_iter()
        .filter(|client| match client {
            SetupClient::Claude => {
                find_on_path("claude").is_some()
                    || home_has(".claude")
                    || root.join(".claude").is_dir()
            }
            SetupClient::Cursor => {
                find_on_path("cursor").is_some()
                    || home_has(".cursor")
                    || root.join(".cursor").is_dir()
            }
            SetupClient::Codex => {
                find_on_path("codex").is_some()
                    || home_has(".codex")
                    || root.join(".codex").is_dir()
            }
            SetupClient::Opencode => {
                find_on_path("opencode").is_some()
                    || home_has(".config/opencode")
                    || root.join("opencode.json").exists()
            }
            SetupClient::Mattpocock => {
                root.join("docs/agents").is_dir()
                    || root
                        .join(".agents/skills/setup-matt-pocock-skills")
                        .is_dir()
                    || home_has(".agents/skills/setup-matt-pocock-skills")
            }
            SetupClient::All => false,
        })
        .collect()
}

pub fn parse_agents(selector: &str, root: &Path) -> Result<Vec<SetupClient>> {
    let selector = selector.trim();
    match selector {
        "auto" => Ok(detect(root)),
        "all" => Ok(SetupClient::all()),
        "none" => Ok(Vec::new()),
        "ask" => prompt_agents(&detect(root)),
        "" => Err(OrbitError::invalid_argument(
            "`--agents` must be auto, all, none, ask, or a comma-separated list",
        )),
        _ => {
            let mut clients = Vec::new();
            for token in
                selector.split(|character: char| character == ',' || character.is_whitespace())
            {
                if token.is_empty() {
                    continue;
                }
                let client = SetupClient::from_name(token).ok_or_else(|| {
                    OrbitError::invalid_argument(format!(
                        "unknown harness `{token}`; expected claude, cursor, codex, opencode, or mattpocock"
                    ))
                })?;
                if client == SetupClient::All {
                    return Ok(SetupClient::all());
                }
                if !clients.contains(&client) {
                    clients.push(client);
                }
            }
            Ok(clients)
        }
    }
}

pub fn selected_agents(args: &crate::cli::InitArgs, root: &Path) -> Result<Vec<SetupClient>> {
    if args.no_agents {
        return Ok(Vec::new());
    }
    if let Some(selector) = &args.agents {
        return parse_agents(selector, root);
    }
    if std::io::stdin().is_terminal() {
        prompt_agents(&detect(root))
    } else {
        Ok(detect(root))
    }
}

fn prompt_agents(detected: &[SetupClient]) -> Result<Vec<SetupClient>> {
    let all = SetupClient::all();
    eprintln!("Orbit supports:");
    for (index, client) in all.iter().enumerate() {
        let mark = if detected.contains(client) {
            " (detected)"
        } else {
            ""
        };
        eprintln!("  {}. {}{mark}", index + 1, client.name());
    }
    eprintln!("Wire which harnesses? [Enter = detected, a = all, none = none]");

    loop {
        eprint!("> ");
        std::io::stderr()
            .flush()
            .map_err(|error| OrbitError::io("flush the prompt", error))?;
        let mut line = String::new();
        if std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| OrbitError::io("read the harness selection", error))?
            == 0
        {
            return Ok(detected.to_vec());
        }

        let answer = line.trim();
        if answer.is_empty() {
            return Ok(detected.to_vec());
        }
        if answer.eq_ignore_ascii_case("a") || answer.eq_ignore_ascii_case("all") {
            return Ok(all);
        }
        if answer.eq_ignore_ascii_case("none") {
            return Ok(Vec::new());
        }

        let mut selected = Vec::new();
        let mut unknown = None;
        for token in answer.split([',', ' ', '\t']) {
            if token.is_empty() {
                continue;
            }
            let client = token
                .parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1))
                .and_then(|index| all.get(index).copied())
                .or_else(|| SetupClient::from_name(token));
            match client {
                Some(client) => {
                    if !selected.contains(&client) {
                        selected.push(client);
                    }
                }
                None => {
                    unknown = Some(token.to_string());
                    break;
                }
            }
        }

        if let Some(token) = unknown {
            eprintln!("unknown harness `{token}`; enter a number or a name");
            continue;
        }
        return Ok(selected);
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn project_root() -> Result<PathBuf> {
    match crate::git::repo_root() {
        Ok(root) => Ok(root),
        Err(_) => std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error)),
    }
}

struct PlanOptions<'a> {
    hooks: bool,
    hook_bin: Option<&'a Path>,
    mcp_bin: Option<&'a Path>,
    mcp_dir: Option<&'a Path>,
}

fn plan_install(
    root: &Path,
    client: SetupClient,
    options: &PlanOptions<'_>,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mcp = options.mcp_bin.is_some();
    match client {
        SetupClient::Claude => {
            plan_section(root, "CLAUDE.md", &workflow_section(), plan)?;
            if options.hooks {
                plan_claude_hooks(root, options.hook_bin, false, plan)?;
            }
            if mcp {
                plan_mcp_json(
                    root,
                    ".mcp.json",
                    McpJsonKind::Servers,
                    mcp_servers_entry(
                        options
                            .mcp_bin
                            .expect("MCP binary resolved when MCP is enabled"),
                        options.mcp_dir,
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
                        options
                            .mcp_bin
                            .expect("MCP binary resolved when MCP is enabled"),
                        options.mcp_dir,
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
                    options
                        .mcp_bin
                        .expect("MCP binary resolved when MCP is enabled"),
                    mcp_args(options.mcp_dir),
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
                    opencode_entry(
                        options
                            .mcp_bin
                            .expect("MCP binary resolved when MCP is enabled"),
                        options.mcp_dir,
                    ),
                    false,
                    plan,
                    warnings,
                )?;
            }
        }
        SetupClient::Mattpocock => {
            plan_section_unless_custom(
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
            plan_claude_hooks(root, None, true, plan)?;
            plan_mcp_json(
                root,
                ".mcp.json",
                McpJsonKind::Servers,
                mcp_servers_entry(Path::new("orbit"), mcp_dir),
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
                mcp_servers_entry(Path::new("orbit"), mcp_dir),
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
                Path::new("orbit"),
                Vec::new(),
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
                opencode_entry(Path::new("orbit"), mcp_dir),
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

/// Install a managed section only when the file is absent or already managed.
/// A hand-written file is reported as skipped instead of gaining a second section.
fn plan_section_unless_custom(
    root: &Path,
    relative: &str,
    section: &str,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    if let Some(existing) = existing.as_deref()
        && !existing.contains(BEGIN_MARKER)
        && !existing.contains(END_MARKER)
    {
        return insert_plan(plan, skipped_plan(absolute, PathBuf::from(relative)));
    }
    plan_section(root, relative, section, plan)
}

fn skipped_plan(absolute: PathBuf, relative: PathBuf) -> FilePlan {
    let warning = format!(
        "{} is not managed by Orbit; leaving it in place",
        relative.display()
    );
    FilePlan {
        absolute,
        relative,
        action: Action::Skipped,
        content: None,
        warning: Some(warning),
    }
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
        insert_plan(plan, skipped_plan(absolute, PathBuf::from(relative)))?;
    }
    Ok(())
}

fn plan_claude_hooks(
    root: &Path,
    bin: Option<&Path>,
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
        let bin = bin.ok_or_else(|| OrbitError::config("hooks require a resolved Orbit binary"))?;
        add_orbit_hook(&mut value, bin)?
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

fn mcp_servers_entry(bin: &Path, dir: Option<&Path>) -> Value {
    json!({
        "command": bin.to_string_lossy(),
        "args": mcp_args(dir)
    })
}

fn opencode_entry(bin: &Path, dir: Option<&Path>) -> Value {
    let mut command = vec![bin.to_string_lossy().to_string()];
    command.extend(mcp_args(dir));
    json!({
        "type": "local",
        "command": command
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
    let layout = kind.layout();
    let container_path = layout.container_path;
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
    let absolute_command =
        entry_command(&entry).is_some_and(|command| Path::new(command).is_absolute());

    if remove {
        for path in std::iter::once(container_path).chain(layout.legacy_container_path) {
            if let Some(container_value) = nested_object_mut(object, path)
                && let Some(existing_entry) = container_value.get("orbit")
            {
                if is_orbit_mcp_entry(existing_entry) {
                    container_value.remove("orbit");
                    changed = true;
                } else {
                    warning = Some(format!(
                        "{relative} has an unrelated `orbit` server; leaving it"
                    ));
                }
            }
        }
        prune_empty_object_path(object, container_path);
    } else {
        if existing.is_none()
            && let Some(schema) = layout.schema
        {
            object.insert("$schema".to_string(), json!(schema));
        }
        if let Some(legacy_path) = layout.legacy_container_path
            && let Some(legacy) = nested_object_mut(object, legacy_path)
        {
            match legacy.get("orbit").cloned() {
                Some(legacy_entry) if is_orbit_mcp_entry(&legacy_entry) => {
                    legacy.remove("orbit");
                    changed = true;
                }
                Some(_) => {
                    return Err(OrbitError::config(format!(
                        "{relative} already has an `orbit` server that Orbit does not manage; move it first"
                    )));
                }
                None => {}
            }
        }
        let container_value = ensure_nested_object_mut(object, container_path, relative)?;

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

    if !remove && absolute_command && is_git_tracked(root, relative) {
        warnings.push(format!(
            "{relative} is tracked by git and will contain a machine-specific MCP command; keep it untracked or pass `--mcp-bin orbit` for a portable config"
        ));
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
    bin: &Path,
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
        let expected = toml_mcp_entry(bin, &args);
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

    if !remove && bin.is_absolute() && is_git_tracked(root, relative) {
        warnings.push(format!(
            "{relative} is tracked by git and will contain a machine-specific MCP command; keep it untracked or pass `--mcp-bin orbit` for a portable config"
        ));
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

fn toml_mcp_entry(bin: &Path, args: &[String]) -> toml::Value {
    let mut table = toml::Table::new();
    table.insert(
        "command".to_string(),
        toml::Value::String(bin.to_string_lossy().to_string()),
    );
    table.insert(
        "args".to_string(),
        toml::Value::Array(args.iter().cloned().map(toml::Value::String).collect()),
    );
    toml::Value::Table(table)
}

fn entry_command(entry: &Value) -> Option<&str> {
    entry.get("command").and_then(Value::as_str).or_else(|| {
        entry
            .get("command")
            .and_then(Value::as_array)
            .and_then(|command| command.first())
            .and_then(Value::as_str)
    })
}

fn is_git_tracked(root: &Path, relative: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--error-unmatch", "--", relative])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn is_orbit_mcp_entry(entry: &Value) -> bool {
    let command_is_orbit = entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_orbit_command)
        || entry
            .get("command")
            .and_then(Value::as_array)
            .and_then(|command| command.first())
            .and_then(Value::as_str)
            .is_some_and(is_orbit_command);

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
        .is_some_and(is_orbit_command);
    let has_mcp = entry
        .get("args")
        .and_then(toml::Value::as_array)
        .is_some_and(|args| args.iter().any(|arg| arg.as_str() == Some("mcp")));
    command_is_orbit && has_mcp
}

fn is_orbit_command(command: &str) -> bool {
    if command == "orbit" || command == "orbit.exe" {
        return true;
    }
    let Some(name) = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    #[cfg(windows)]
    {
        name.eq_ignore_ascii_case("orbit") || name.eq_ignore_ascii_case("orbit.exe")
    }
    #[cfg(not(windows))]
    {
        name == "orbit" || name == "orbit.exe"
    }
}

pub(crate) fn resolve_mcp_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.components().count() == 1 {
            if !is_orbit_command(&path.to_string_lossy()) {
                return Err(OrbitError::config(
                    "`--mcp-bin` must name `orbit` or `orbit.exe`",
                ));
            }
            return Ok(path.to_path_buf());
        }
        let absolute = absolute_executable(path)?;
        if !is_orbit_command(&absolute.to_string_lossy()) {
            return Err(OrbitError::config(
                "`--mcp-bin` must point to an executable named `orbit` or `orbit.exe`",
            ));
        }
        reject_ephemeral(&absolute)?;
        return Ok(absolute);
    }

    if let Some(arg0) = std::env::args_os().next().map(PathBuf::from)
        && arg0.components().count() > 1
    {
        let absolute = absolute_executable(&arg0)?;
        if !is_orbit_command(&absolute.to_string_lossy()) {
            return Err(OrbitError::config(format!(
                "the running executable is named `{}`; rename it to `orbit` or pass `--mcp-bin`",
                absolute
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default()
            )));
        }
        reject_ephemeral(&absolute)?;
        return Ok(absolute);
    }

    if let Some(found) = find_on_path(binary_name()) {
        reject_ephemeral(&found)?;
        return Ok(found);
    }

    let current = std::env::current_exe()
        .map_err(|error| OrbitError::io("resolve the orbit executable", error))?;
    if !is_executable(&current) {
        return Err(OrbitError::config(format!(
            "the running executable is not accessible: {}",
            current.display()
        )));
    }
    reject_ephemeral(&current)?;
    Ok(current)
}

fn reject_ephemeral(path: &Path) -> Result<()> {
    let in_npx_cache = path
        .components()
        .any(|component| component.as_os_str() == "_npx");
    let mut temp_roots = vec![std::env::temp_dir()];
    #[cfg(unix)]
    {
        for root in ["/tmp", "/private/tmp", "/var/tmp", "/private/var/tmp"] {
            temp_roots.push(PathBuf::from(root));
        }
    }
    let in_temp = temp_roots.iter().any(|root| path.starts_with(root));
    if in_npx_cache || in_temp {
        return Err(OrbitError::new(
            code::EPHEMERAL_BIN,
            format!(
                "refusing to record the ephemeral path {}; run `npm install -g @rcmendes/orbit` or pass `--mcp-bin orbit`",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn absolute_executable(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .map_err(|error| OrbitError::io(format!("resolve {}", path.display()), error))?;
    if !is_executable(&absolute) {
        return Err(OrbitError::config(format!(
            "{} is not an executable file",
            absolute.display()
        )));
    }
    Ok(absolute)
}

pub(crate) fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = if dir.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            dir.join(name)
        };
        let Ok(absolute) = std::path::absolute(&candidate) else {
            continue;
        };
        if is_executable(&absolute) {
            return Some(absolute);
        }
    }
    None
}

fn binary_name() -> &'static str {
    if cfg!(windows) { "orbit.exe" } else { "orbit" }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn is_build_artifact(path: &Path) -> bool {
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        if component.as_os_str() == "target"
            && let Some(next) = components.peek()
            && (next.as_os_str() == "debug" || next.as_os_str() == "release")
        {
            return true;
        }
    }
    false
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
            let after = existing[end..]
                .trim_start_matches('\n')
                .trim_end_matches('\n');
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

fn add_orbit_hook(value: &mut Value, bin: &Path) -> Result<bool> {
    let desired = hook_command(bin);
    {
        let object = value.as_object().ok_or_else(|| {
            OrbitError::config(".claude/settings.json must contain a JSON object")
        })?;
        if let Some(groups) = object
            .get("hooks")
            .and_then(|hooks| hooks.get("SessionStart"))
            .and_then(Value::as_array)
            && groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|handlers| {
                        handlers.iter().any(|handler| {
                            handler.get("command").and_then(Value::as_str) == Some(desired.as_str())
                        })
                    })
            })
        {
            return Ok(false);
        }
    }

    // Migrate away from a bare `orbit` command or a previous binary path.
    remove_orbit_hook(value)?;

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

    session_start.push(json!({
        "matcher": "",
        "hooks": [
            { "type": "command", "command": desired }
        ]
    }));
    Ok(true)
}

fn hook_command(bin: &Path) -> String {
    let rendered = bin.to_string_lossy();
    let rendered = if rendered.contains(' ') {
        format!("\"{rendered}\"")
    } else {
        rendered.to_string()
    };
    format!("{rendered} prime --hook-json --by claude")
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
{COMPLETION_LINE}
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
- To finish or abandon work, run `orbit done <ID> --by <agent> --comment \"...\"` or `orbit cancel <ID> --by <agent>`; the note is recorded with the status change.
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
    fn checked_in_agents_md_matches_the_generated_section() {
        let agents = include_str!("../AGENTS.md");
        assert!(
            agents.contains(workflow_section().as_str()),
            "AGENTS.md is stale; run `orbit setup opencode --no-mcp` to refresh the managed block"
        );
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
    fn upsert_does_not_accumulate_trailing_blank_lines() {
        let section = workflow_section();
        let existing = format!("Before.\n\n{section}\n\nAfter.\n\n\n");
        let first = upsert_section(&existing, &section, "AGENTS.md").unwrap();
        let second = upsert_section(&first, &section, "AGENTS.md").unwrap();
        assert_eq!(first, second);
        assert!(first.ends_with("After.\n"));

        let only_section =
            upsert_section(&format!("{section}\n\n\n"), &section, "AGENTS.md").unwrap();
        let again = upsert_section(&only_section, &section, "AGENTS.md").unwrap();
        assert_eq!(only_section, again);
        assert_eq!(only_section, format!("{section}\n"));
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
        let bin = Path::new("/opt/orbit/bin/orbit");
        assert!(add_orbit_hook(&mut value, bin).unwrap());
        assert!(!add_orbit_hook(&mut value, bin).unwrap());
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "/opt/orbit/bin/orbit prime --hook-json --by claude"
        );
        assert_eq!(value["permissions"]["allow"][0], "Bash(git status)");

        assert!(remove_orbit_hook(&mut value).unwrap());
        assert!(value.get("hooks").is_none());
        assert_eq!(value["permissions"]["allow"][0], "Bash(git status)");
    }
}
