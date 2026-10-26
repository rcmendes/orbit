use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
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

    pub fn uses_mcp(self) -> bool {
        !matches!(self, SetupClient::Mattpocock | SetupClient::All)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SetupScope {
    Project,
    User,
}

impl SetupScope {
    pub fn name(self) -> &'static str {
        match self {
            SetupScope::Project => "project",
            SetupScope::User => "user",
        }
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
    pub scope: SetupScope,
    pub clients: Vec<SetupClient>,
    pub hooks: bool,
    pub hook_bin: Option<PathBuf>,
    pub mcp_bin: Option<PathBuf>,
    pub print: bool,
    pub remove: bool,
}

pub fn run(args: crate::cli::SetupArgs, _dir: Option<&Path>) -> Result<CommandOutput> {
    let project = project_root()?;
    let root = scope_root(args.scope, &project)?;
    let mut warnings = Vec::new();

    if matches!(args.client, SetupClient::Mattpocock)
        && (args.mcp || args.mcp_bin.is_some() || args.portable_mcp)
    {
        warnings.push("mattpocock is not an MCP client; ignoring MCP options".to_string());
    }

    let hooks =
        args.hooks && !args.remove && matches!(args.client, SetupClient::Claude | SetupClient::All);
    if args.hooks && !args.remove && !hooks {
        warnings.push("hooks are only supported for Claude in M1; ignoring --hooks".to_string());
    }

    let mcp = !args.no_mcp;
    let clients = expand(args.client);
    let mcp_bin = if mcp && !args.remove {
        Some(configured_mcp_bin(
            args.portable_mcp,
            args.mcp_bin.as_deref(),
        )?)
    } else {
        None
    };
    let hook_bin = if hooks {
        Some(configured_hook_bin(args.mcp_bin.as_deref())?)
    } else {
        None
    };

    if args.check {
        let preview = install(
            &root,
            InstallOptions {
                scope: args.scope,
                clients,
                hooks,
                hook_bin: hook_bin.clone(),
                mcp_bin: mcp_bin.clone(),
                print: true,
                remove: false,
            },
        )?;
        let mut output = check_output(&root, args.scope, preview);
        output.warnings.extend(warnings);
        return Ok(output);
    }

    let mut output = install(
        &root,
        InstallOptions {
            scope: args.scope,
            clients,
            hooks,
            hook_bin,
            mcp_bin,
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
    let has_mcp_client = options.clients.iter().any(|client| client.uses_mcp());
    if has_mcp_client && let Some(bin) = options.mcp_bin.as_deref() {
        if bin.components().count() == 1 {
            warnings.push(
                "MCP config uses the portable `orbit` command; install Orbit and make sure the editor or agent that launches it can find `orbit` on PATH"
                    .to_string(),
            );
        } else if is_build_artifact(bin) {
            warnings.push(format!(
                "agent config points at a build artifact ({}); install Orbit to a stable location or re-run setup after rebuilding",
                bin.display()
            ));
        }
    }

    for client in options.clients {
        if options.remove {
            plan_remove(root, client, &mut plan, &mut warnings, options.scope)?;
        } else {
            plan_install(
                root,
                client,
                &PlanOptions {
                    scope: options.scope,
                    hooks: options.hooks,
                    hook_bin: options.hook_bin.as_deref(),
                    mcp_bin: options.mcp_bin.as_deref(),
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

    let human = render_human(root, options.print, &files, options.scope);
    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "scope": options.scope.name(),
            "preview": options.print,
            "files": file_values,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

pub fn refresh(root: &Path) -> Result<CommandOutput> {
    let clients = configured_clients(root);
    if clients.is_empty() {
        return Ok(CommandOutput::new(
            json!({
                "root": root.to_string_lossy(),
                "scope": SetupScope::Project.name(),
                "preview": false,
                "files": [],
            }),
            "no agent integration to refresh\n",
        ));
    }

    let mut files = BTreeMap::new();
    let mut warnings = Vec::new();
    for client in clients {
        let portable = configured_portable_mcp(root, client).unwrap_or(false);
        let bin = configured_mcp_bin(portable, None)?;
        let output = install(
            root,
            InstallOptions {
                scope: SetupScope::Project,
                clients: vec![client],
                hooks: false,
                hook_bin: None,
                mcp_bin: Some(bin),
                print: false,
                remove: false,
            },
        )?;
        if let Some(installed) = output.data["files"].as_array() {
            for file in installed {
                let Some(path) = file["path"].as_str().map(str::to_string) else {
                    continue;
                };
                let entry = files.entry(path).or_insert_with(|| file.clone());
                if entry["action"] == "unchanged" && file["action"] != "unchanged" {
                    *entry = file.clone();
                }
            }
        }
        warnings.extend(output.warnings);
    }

    let files: Vec<Value> = files.into_values().collect();
    let mut human = String::new();
    for file in &files {
        human.push_str(&format!(
            "{} {}\n",
            file["action"].as_str().unwrap_or("unknown"),
            file["path"].as_str().unwrap_or("")
        ));
    }
    warnings.sort();
    warnings.dedup();
    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "preview": false,
            "files": files,
        }),
        human,
    );
    output.warnings = warnings;
    Ok(output)
}

pub(crate) fn has_orbit_hook(root: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(root.join(".claude/settings.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return false;
    };
    value
        .get("hooks")
        .and_then(|hooks| hooks.get("SessionStart"))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|handlers| handlers.iter().any(is_orbit_hook))
            })
        })
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

/// Report whether the selected harnesses already have an Orbit MCP entry.
fn has_configured_mcp(root: &Path, clients: &[SetupClient]) -> bool {
    clients
        .iter()
        .any(|client| configured_mcp_command(root, *client).is_some())
}

fn configured_portable_mcp(root: &Path, client: SetupClient) -> Option<bool> {
    configured_mcp_command(root, client).map(|command| is_portable_mcp_bin(Path::new(&command)))
}

fn configured_mcp_command(root: &Path, client: SetupClient) -> Option<String> {
    match client {
        SetupClient::Claude => read_json_orbit_command(root, ".mcp.json", &["mcpServers", "orbit"]),
        SetupClient::Cursor => {
            read_json_orbit_command(root, ".cursor/mcp.json", &["mcpServers", "orbit"])
        }
        SetupClient::Codex => read_toml_orbit_command(root, ".codex/config.toml"),
        SetupClient::Opencode => {
            read_json_orbit_command(root, "opencode.json", &["mcp", "servers", "orbit"])
                .or_else(|| read_json_orbit_command(root, "opencode.json", &["mcp", "orbit"]))
        }
        SetupClient::Mattpocock | SetupClient::All => None,
    }
}

fn read_json_orbit_command(root: &Path, relative: &str, path: &[&str]) -> Option<String> {
    let content = std::fs::read_to_string(root.join(relative)).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    let mut current = &value;
    for key in path {
        current = current.get(*key)?;
    }
    entry_command(current).map(str::to_string)
}

fn read_toml_orbit_command(root: &Path, relative: &str) -> Option<String> {
    let content = std::fs::read_to_string(root.join(relative)).ok()?;
    let value: toml::Value = toml::from_str(&content).ok()?;
    value
        .get("mcp_servers")?
        .get("orbit")?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

fn check_output(root: &Path, scope: SetupScope, preview: CommandOutput) -> CommandOutput {
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
            display_path(root, scope, file["path"].as_str().unwrap_or(""))
        ));
    }

    let mut output = CommandOutput::new(
        json!({
            "root": root.to_string_lossy(),
            "scope": scope.name(),
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
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut prompt = std::io::stderr();
    parse_agents_with(selector, root, &mut input, &mut prompt)
}

fn parse_agents_with(
    selector: &str,
    root: &Path,
    input: &mut impl BufRead,
    prompt: &mut impl Write,
) -> Result<Vec<SetupClient>> {
    let selector = selector.trim();
    match selector {
        "auto" => Ok(detect(root)),
        "all" => Ok(SetupClient::all()),
        "none" => Ok(Vec::new()),
        "ask" => prompt_agents(&detect(root), input, prompt),
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
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let mut input = stdin.lock();
    let mut prompt = std::io::stderr();
    select_agents(args, root, interactive, &mut input, &mut prompt)
}

/// Choose the MCP command for first-run setup. An explicit flag wins, an
/// existing managed configuration skips the prompt, and a terminal gets the
/// choice with the absolute path as the default.
pub fn selected_portable_mcp(
    args: &crate::cli::InitArgs,
    clients: &[SetupClient],
    root: &Path,
) -> Result<bool> {
    let stdin = std::io::stdin();
    let interactive = stdin.is_terminal();
    let mut input = stdin.lock();
    let mut prompt = std::io::stderr();
    select_portable_mcp(args, clients, root, interactive, &mut input, &mut prompt)
}

fn select_portable_mcp(
    args: &crate::cli::InitArgs,
    clients: &[SetupClient],
    root: &Path,
    interactive: bool,
    input: &mut impl BufRead,
    prompt: &mut impl Write,
) -> Result<bool> {
    if !clients.iter().any(|client| client.uses_mcp()) {
        return Ok(false);
    }
    if args.portable_mcp {
        return Ok(true);
    }
    if args.mcp_bin.is_some() {
        return Ok(false);
    }
    if args.print || !interactive || has_configured_mcp(root, clients) {
        return Ok(false);
    }
    prompt_portable_mcp(input, prompt)
}

fn prompt_portable_mcp(input: &mut impl BufRead, prompt: &mut impl Write) -> Result<bool> {
    writeln!(prompt, "MCP configuration:")
        .map_err(|error| OrbitError::io("write the MCP prompt", error))?;
    writeln!(
        prompt,
        "  1. absolute path (default): records this Orbit binary; GUI-launched editors need no PATH setup"
    )
    .map_err(|error| OrbitError::io("write the MCP prompt", error))?;
    writeln!(
        prompt,
        "  2. portable: writes `orbit`; every editor that launches it must find Orbit on PATH"
    )
    .map_err(|error| OrbitError::io("write the MCP prompt", error))?;
    writeln!(
        prompt,
        "Use which MCP command? [Enter = absolute, p = portable]"
    )
    .map_err(|error| OrbitError::io("write the MCP prompt", error))?;

    loop {
        write!(prompt, "> ").map_err(|error| OrbitError::io("write the MCP prompt", error))?;
        prompt
            .flush()
            .map_err(|error| OrbitError::io("flush the MCP prompt", error))?;
        let mut line = String::new();
        if input
            .read_line(&mut line)
            .map_err(|error| OrbitError::io("read the MCP choice", error))?
            == 0
        {
            return Ok(false);
        }

        let answer = line.trim();
        if answer.is_empty()
            || answer == "1"
            || answer.eq_ignore_ascii_case("absolute")
            || answer.eq_ignore_ascii_case("a")
        {
            return Ok(false);
        }
        if answer == "2"
            || answer.eq_ignore_ascii_case("portable")
            || answer.eq_ignore_ascii_case("p")
        {
            return Ok(true);
        }
        writeln!(
            prompt,
            "unknown MCP choice `{answer}`; enter 1 for absolute or 2 for portable"
        )
        .map_err(|error| OrbitError::io("write the MCP prompt", error))?;
    }
}

/// Preview runs must never block on input, so they use detection even in a
/// terminal and never prompt for `--agents ask`. Interactive `init` keeps its
/// prompt; a non-interactive run writes no agent files.
fn select_agents(
    args: &crate::cli::InitArgs,
    root: &Path,
    interactive: bool,
    input: &mut impl BufRead,
    prompt: &mut impl Write,
) -> Result<Vec<SetupClient>> {
    if args.no_agents {
        return Ok(Vec::new());
    }
    if let Some(selector) = &args.agents {
        if selector.trim() == "ask" && args.print {
            return Ok(detect(root));
        }
        return parse_agents_with(selector, root, input, prompt);
    }
    if args.print {
        return Ok(detect(root));
    }
    if interactive {
        return prompt_agents(&detect(root), input, prompt);
    }
    Ok(Vec::new())
}

fn prompt_agents(
    detected: &[SetupClient],
    input: &mut impl BufRead,
    prompt: &mut impl Write,
) -> Result<Vec<SetupClient>> {
    let all = SetupClient::all();
    writeln!(prompt, "Orbit supports:")
        .map_err(|error| OrbitError::io("write the harness prompt", error))?;
    for (index, client) in all.iter().enumerate() {
        let mark = if detected.contains(client) {
            " (detected)"
        } else {
            ""
        };
        writeln!(prompt, "  {}. {}{mark}", index + 1, client.name())
            .map_err(|error| OrbitError::io("write the harness prompt", error))?;
    }
    writeln!(
        prompt,
        "Wire which harnesses? [Enter = detected, a = all, none = none]"
    )
    .map_err(|error| OrbitError::io("write the harness prompt", error))?;

    loop {
        write!(prompt, "> ").map_err(|error| OrbitError::io("write the harness prompt", error))?;
        prompt
            .flush()
            .map_err(|error| OrbitError::io("flush the prompt", error))?;
        let mut line = String::new();
        if input
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
            writeln!(
                prompt,
                "unknown harness `{token}`; enter a number or a name"
            )
            .map_err(|error| OrbitError::io("write the harness prompt", error))?;
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

fn scope_root(scope: SetupScope, project: &Path) -> Result<PathBuf> {
    match scope {
        SetupScope::Project => Ok(project.to_path_buf()),
        SetupScope::User => home_dir()
            .map(|home| dunce::canonicalize(&home).unwrap_or(home))
            .ok_or_else(|| {
                OrbitError::config("HOME or USERPROFILE must be set for `setup --scope user`")
            }),
    }
}

fn project_root() -> Result<PathBuf> {
    match crate::git::repo_root() {
        Ok(root) => Ok(root),
        Err(_) => std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error)),
    }
}

struct PlanOptions<'a> {
    scope: SetupScope,
    hooks: bool,
    hook_bin: Option<&'a Path>,
    mcp_bin: Option<&'a Path>,
}

#[derive(Default)]
struct ClientPaths {
    instructions: Option<&'static str>,
    rule: Option<&'static str>,
    skill: Option<&'static str>,
    hooks: Option<&'static str>,
    mcp_json: Option<(&'static str, McpJsonKind)>,
    mcp_toml: Option<&'static str>,
    tracker: Option<&'static str>,
}

fn client_paths(client: SetupClient, scope: SetupScope) -> ClientPaths {
    let user = scope == SetupScope::User;
    match client {
        SetupClient::Claude => ClientPaths {
            instructions: Some(if user {
                ".claude/CLAUDE.md"
            } else {
                "CLAUDE.md"
            }),
            hooks: Some(".claude/settings.json"),
            mcp_json: Some((
                if user { ".claude.json" } else { ".mcp.json" },
                McpJsonKind::Servers,
            )),
            ..ClientPaths::default()
        },
        SetupClient::Cursor => ClientPaths {
            rule: Some(".cursor/rules/orbit.mdc"),
            skill: Some(".agents/skills/orbit/SKILL.md"),
            mcp_json: Some((".cursor/mcp.json", McpJsonKind::Servers)),
            ..ClientPaths::default()
        },
        SetupClient::Codex => ClientPaths {
            instructions: Some(if user {
                ".codex/AGENTS.md"
            } else {
                "AGENTS.md"
            }),
            skill: Some(".agents/skills/orbit/SKILL.md"),
            mcp_toml: Some(".codex/config.toml"),
            ..ClientPaths::default()
        },
        SetupClient::Opencode => ClientPaths {
            instructions: Some(if user {
                ".config/opencode/AGENTS.md"
            } else {
                "AGENTS.md"
            }),
            mcp_json: Some((
                if user {
                    ".config/opencode/opencode.json"
                } else {
                    "opencode.json"
                },
                McpJsonKind::Opencode,
            )),
            ..ClientPaths::default()
        },
        SetupClient::Mattpocock => ClientPaths {
            tracker: Some(if user {
                ".agents/issue-tracker.md"
            } else {
                "docs/agents/issue-tracker.md"
            }),
            ..ClientPaths::default()
        },
        SetupClient::All => unreachable!("All is expanded before planning"),
    }
}

#[derive(Clone, Copy)]
struct PlanTarget<'a> {
    root: &'a Path,
    scope: SetupScope,
}

fn plan_install(
    root: &Path,
    client: SetupClient,
    options: &PlanOptions<'_>,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mcp = options.mcp_bin.is_some();
    let paths = client_paths(client, options.scope);
    let target = PlanTarget {
        root,
        scope: options.scope,
    };
    let bin = || {
        options
            .mcp_bin
            .expect("MCP binary resolved when MCP is enabled")
    };

    if let Some(instructions) = paths.instructions {
        plan_section(root, instructions, &workflow_section(), plan)?;
    }
    if let Some(rule) = paths.rule {
        plan_full_file(root, rule, &cursor_rule(), plan)?;
    }
    if let Some(skill) = paths.skill {
        plan_full_file(root, skill, &skill_file(), plan)?;
    }
    if let Some(tracker) = paths.tracker {
        plan_section_unless_custom(root, tracker, &mattpocock_section(), plan)?;
    }
    if options.hooks
        && let Some(hooks) = paths.hooks
    {
        plan_claude_hooks(root, hooks, options.hook_bin, false, plan)?;
    }
    if mcp {
        if let Some((relative, kind)) = paths.mcp_json {
            let entry = match kind {
                McpJsonKind::Servers => mcp_servers_entry(bin()),
                McpJsonKind::Opencode => opencode_entry(bin()),
            };
            plan_mcp_json(target, relative, kind, entry, false, plan, warnings)?;
        }
        if let Some(relative) = paths.mcp_toml {
            plan_mcp_toml(target, relative, bin(), mcp_args(), false, plan, warnings)?;
        }
    }
    Ok(())
}

fn plan_remove(
    root: &Path,
    client: SetupClient,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
    scope: SetupScope,
) -> Result<()> {
    let paths = client_paths(client, scope);
    let orbit = Path::new("orbit");
    let target = PlanTarget { root, scope };

    if let Some(instructions) = paths.instructions {
        plan_section_remove(root, instructions, plan)?;
    }
    if let Some(rule) = paths.rule {
        plan_full_file_remove(root, rule, plan)?;
    }
    if let Some(skill) = paths.skill {
        plan_full_file_remove(root, skill, plan)?;
    }
    if let Some(tracker) = paths.tracker {
        plan_section_remove(root, tracker, plan)?;
    }
    if let Some(hooks) = paths.hooks {
        plan_claude_hooks(root, hooks, None, true, plan)?;
    }
    if let Some((relative, kind)) = paths.mcp_json {
        let entry = match kind {
            McpJsonKind::Servers => mcp_servers_entry(orbit),
            McpJsonKind::Opencode => opencode_entry(orbit),
        };
        plan_mcp_json(target, relative, kind, entry, true, plan, warnings)?;
    }
    if let Some(relative) = paths.mcp_toml {
        plan_mcp_toml(target, relative, orbit, Vec::new(), true, plan, warnings)?;
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
    relative: &str,
    bin: Option<&Path>,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
) -> Result<()> {
    let absolute = root.join(relative);
    let existing = read_optional(&absolute)?;
    let mut value: Value = match &existing {
        Some(content) => serde_json::from_str(content)
            .map_err(|error| OrbitError::config(format!("invalid {relative}: {error}")))?,
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

fn mcp_args() -> Vec<String> {
    vec!["mcp".to_string()]
}

fn mcp_servers_entry(bin: &Path) -> Value {
    json!({
        "command": bin.to_string_lossy(),
        "args": mcp_args()
    })
}

fn opencode_entry(bin: &Path) -> Value {
    let mut command = vec![bin.to_string_lossy().to_string()];
    command.extend(mcp_args());
    json!({
        "type": "local",
        "command": command
    })
}

fn plan_mcp_json(
    target: PlanTarget<'_>,
    relative: &str,
    kind: McpJsonKind,
    entry: Value,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let PlanTarget { root, scope } = target;
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

    if !remove && scope == SetupScope::Project && absolute_command && is_git_tracked(root, relative)
    {
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
    target: PlanTarget<'_>,
    relative: &str,
    bin: &Path,
    args: Vec<String>,
    remove: bool,
    plan: &mut BTreeMap<PathBuf, FilePlan>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let PlanTarget { root, scope } = target;
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

    if !remove
        && scope == SetupScope::Project
        && bin.is_absolute()
        && is_git_tracked(root, relative)
    {
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

pub(crate) fn is_portable_mcp_bin(path: &Path) -> bool {
    path.components().count() == 1 && is_orbit_command(&path.to_string_lossy())
}

pub(crate) fn configured_mcp_bin(portable: bool, explicit: Option<&Path>) -> Result<PathBuf> {
    if portable {
        Ok(PathBuf::from("orbit"))
    } else {
        resolve_mcp_bin(explicit)
    }
}

pub(crate) fn configured_hook_bin(explicit: Option<&Path>) -> Result<PathBuf> {
    let resolved = resolve_mcp_bin(explicit)?;
    if is_portable_mcp_bin(&resolved) {
        resolve_mcp_bin(None)
    } else {
        Ok(resolved)
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
    #[cfg(unix)]
    let temp_roots = [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/private/var/tmp"),
    ];
    #[cfg(not(unix))]
    let temp_roots = [std::env::temp_dir()];
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
    let Some(command) = handler.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some((binary, args)) = command.split_once(" prime --hook-json") else {
        return false;
    };
    if !args.starts_with(" --by") {
        return false;
    }
    Path::new(binary.trim_matches('"'))
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "orbit" || name == "orbit.exe")
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
- `orbit add \"title\" --parent <id>` creates a SubTask. A spec or epic is a top-level Task with `--type epic`; its issues are SubTasks. `type` is metadata: task, epic, spike, chore, decision. Epic is top-level only. SubTasks are one level deep and the parent waits for them.
- Never use `done --force` or `cancel --force` on a parent with open SubTasks; it cancels them.
- Use `--type epic` or `--exclude-type epic` on `orbit ready` or `orbit list` to find or skip spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Use `orbit tree <id>` to inspect one epic/spec with its SubTasks and blockers.
- In a multi-repository workspace, add `--repo <name>` to `orbit ready` or `orbit list`.
- `orbit depend <id> <dep-id>` links work; cycles and unfinished dependencies are rejected.
- `orbit show <id>` reads the full body and acceptance criteria.
- Use `orbit label add/remove` for triage labels and `orbit comment add/list` for durable notes.
- Run `orbit prime` at session start for ready work, stale claims, and counts.
- `orbit list` prints a table for humans; parse `--json` in automation. `orbit ready` and `orbit tree` print compact summary lines.

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
- Orbit calls the unit a Task. A skill's \"issue\" is a Task; a \"sub-issue\" is a SubTask; an \"epic\" or \"spec\" is a top-level Task with `--type epic`.
- To publish tickets under an existing issue or spec, run `orbit add \"Title\" --parent <PARENT_ID>`; SubTasks are one level deep and the parent waits for them.
- `type` is metadata: task, epic, spike, chore, decision. Epic is top-level only. `bug` and `enhancement` stay labels.
- Never use `done --force` or `cancel --force` on a parent with open SubTasks; it cancels them.
- Use `--type epic` or `--exclude-type epic` on `orbit ready` or `orbit list` to find or skip spec/epic containers. `--exclude-label epic` remains for legacy boards.
- Use `orbit tree <id>` to inspect one epic/spec with its SubTasks and blockers.
- In a multi-repository workspace, add `--repo <NAME>` to `orbit ready` or `orbit list`.
- To label work, use `orbit label add <ID> <label>...` and `orbit label remove <ID> <label>...`; filter with `orbit list --label <label>` or `orbit list --unlabeled`.
- To comment, use `orbit comment add <ID> --body \"...\" --by <agent>` and `orbit comment list <ID>`.
- The hold labels `needs-triage`, `needs-info`, and `ready-for-human` keep tasks out of the ready queue and prevent claims until removed. `ready-for-agent`, `bug`, and `enhancement` are neutral.
- Never edit task frontmatter directly; that bypasses dependency validation, leases, and the write lock.
- Use `orbit show <ID>` to read acceptance criteria.
- `orbit list` prints a table for humans; parse `--json` in automation. `orbit ready` and `orbit tree` print compact summary lines.
{END_MARKER}"
    )
}

fn display_path(root: &Path, scope: SetupScope, relative: &str) -> String {
    match scope {
        SetupScope::Project => relative.to_string(),
        SetupScope::User => root.join(relative).to_string_lossy().to_string(),
    }
}

fn render_human(root: &Path, preview: bool, files: &[FilePlan], scope: SetupScope) -> String {
    let mut output = String::new();
    if preview {
        output.push_str(&format!("setup preview (root: {})\n", root.display()));
    }
    if files.is_empty() {
        output.push_str("no changes\n");
        return output;
    }

    for file in files {
        let relative = file.relative.to_string_lossy();
        output.push_str(&format!(
            "{} {}\n",
            file.action.as_str(),
            display_path(root, scope, &relative)
        ));
        if preview && let Some(content) = &file.content {
            output.push_str(&format!("--- {relative} ---\n{content}"));
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
        // Windows checkouts may use CRLF; compare normalized text.
        let agents = include_str!("../AGENTS.md").replace("\r\n", "\n");
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

    fn init_args(print: bool, agents: Option<&str>) -> crate::cli::InitArgs {
        crate::cli::InitArgs {
            target: None,
            global: false,
            agents: agents.map(str::to_string),
            no_agents: false,
            hooks: false,
            mcp_bin: None,
            portable_mcp: false,
            slug: None,
            print,
        }
    }

    #[test]
    fn preview_selects_agents_without_reading_input() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"1\n");
        let mut prompt = Vec::new();

        let selected = select_agents(
            &init_args(true, None),
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert_eq!(selected, detect(root.path()));
        assert_eq!(input.position(), 0, "preview consumed stdin");
        assert!(prompt.is_empty(), "preview wrote a prompt");
    }

    #[test]
    fn preview_ask_does_not_read_input_or_prompt() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"1\n");
        let mut prompt = Vec::new();

        let selected = select_agents(
            &init_args(true, Some("ask")),
            root.path(),
            false,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert_eq!(selected, detect(root.path()));
        assert_eq!(input.position(), 0, "preview consumed stdin");
        assert!(prompt.is_empty(), "preview wrote a prompt");
    }

    #[test]
    fn interactive_init_prompts_with_a_terminal() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"\n");
        let mut prompt = Vec::new();

        let selected = select_agents(
            &init_args(false, None),
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert_eq!(selected, detect(root.path()));
        assert!(input.position() > 0);
        assert!(!prompt.is_empty());
    }

    #[test]
    fn non_interactive_init_writes_no_agent_files() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"1\n");
        let mut prompt = Vec::new();

        let selected = select_agents(
            &init_args(false, None),
            root.path(),
            false,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert!(selected.is_empty());
        assert_eq!(input.position(), 0);
        assert!(prompt.is_empty());
    }

    #[test]
    fn explicit_agents_skip_the_prompt_without_a_terminal() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"1\n");
        let mut prompt = Vec::new();

        let selected = select_agents(
            &init_args(false, Some("claude")),
            root.path(),
            false,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert_eq!(selected, vec![SetupClient::Claude]);
        assert_eq!(input.position(), 0);
        assert!(prompt.is_empty());
    }

    #[test]
    fn interactive_init_defaults_the_mcp_choice_to_an_absolute_path() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"\n");
        let mut prompt = Vec::new();

        let portable = select_portable_mcp(
            &init_args(false, None),
            &[SetupClient::Claude],
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert!(!portable);
        assert!(input.position() > 0);
        let prompt = String::from_utf8(prompt).unwrap();
        assert!(prompt.contains("absolute path"), "{prompt}");
        assert!(prompt.contains("PATH"), "{prompt}");
    }

    #[test]
    fn interactive_init_accepts_the_portable_mcp_choice() {
        let root = tempfile::tempdir().unwrap();
        let mut input = std::io::Cursor::new(b"p\n");
        let mut prompt = Vec::new();

        let portable = select_portable_mcp(
            &init_args(false, None),
            &[SetupClient::Opencode],
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert!(portable);
        assert!(input.position() > 0);
        assert!(!prompt.is_empty());
    }

    #[test]
    fn an_explicit_portable_flag_skips_the_mcp_prompt() {
        let root = tempfile::tempdir().unwrap();
        let mut args = init_args(false, None);
        args.portable_mcp = true;
        let mut input = std::io::Cursor::new(b"1\n");
        let mut prompt = Vec::new();

        let portable = select_portable_mcp(
            &args,
            &[SetupClient::Claude],
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert!(portable);
        assert_eq!(input.position(), 0);
        assert!(prompt.is_empty());
    }

    #[test]
    fn an_existing_mcp_config_skips_the_first_run_choice() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(".mcp.json"),
            r#"{"mcpServers":{"orbit":{"command":"orbit","args":["mcp"]}}}"#,
        )
        .unwrap();
        let mut input = std::io::Cursor::new(b"p\n");
        let mut prompt = Vec::new();

        let portable = select_portable_mcp(
            &init_args(false, None),
            &[SetupClient::Claude],
            root.path(),
            true,
            &mut input,
            &mut prompt,
        )
        .unwrap();

        assert!(!portable);
        assert_eq!(input.position(), 0);
        assert!(prompt.is_empty());
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
