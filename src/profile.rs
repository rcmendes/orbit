//! Declarative profiles (ADR 0022): a TOML document that translates a
//! plugin's vocabulary onto the Board — its required and optional
//! capabilities, a workflow template or custom workflow, hold and role
//! names, and vocabulary mappings — and records itself on the Board so
//! `orbit doctor` and `orbit capabilities` can negotiate against it.
//!
//! A profile never adds code: applying one only rewrites `config.toml`
//! sections the kernel already understands (`[workflow]`, `board.*`) and
//! upserts a `[[profiles]]` record. `orbit setup <plugin>` resolves a
//! profile by name — a shipped built-in, a Board-local `profiles/`
//! directory, or `~/.config/orbit/profiles/` — and applies it
//! idempotently under the Board lock.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::board::{Board, Config};
use crate::contract;
use crate::error::{OrbitError, Result, code};
use crate::hierarchy::{HierarchyLevelConfig, IssueTypeConfig};
use crate::task;
use crate::workflow::{Workflow, WorkflowConfig};

/// Profiles shipped inside the Orbit binary, as `(name, TOML)` pairs.
/// Plugin profiles that belong to Orbit itself register here; Boards and
/// users supply their own through the profile search path instead.
const BUILTIN_PROFILES: &[(&str, &str)] = &[("mattpocock", MATPOCOCK_PROFILE)];

/// The `mattpocock` profile: Matt Pocock's engineering skills speak five
/// triage roles (`needs-triage`, `needs-info`, `ready-for-agent`,
/// `ready-for-human`, `wontfix`) plus the `bug`/`enhancement` categories.
/// The profile maps them onto the Issue model — statuses and transitions
/// for the state roles, a human assignee kind for `ready-for-human`, a
/// `Won't Do` resolution for `wontfix`, and the `bug`/`story` Issue types
/// for the categories — and `orbit setup mattpocock` regenerates the
/// `docs/agents/` guides from this vocabulary.
const MATPOCOCK_PROFILE: &str = r#"
name = "mattpocock"
description = "Matt Pocock engineering skills: five triage roles on the Issue workflow"
contract = "1.0.0"
requires = ["workflow", "issue-types", "assignee"]
optional = ["policy"]

[roles]
# `ready-for-human` is ready work for a person: transition to Ready, then
# `orbit assign <id> --to <name>` (assignee kind `human` is the default).
ready-for-human = "human"

[vocabulary.status]
needs-triage = "Triage"
needs-info = "Needs Info"
ready-for-agent = "Ready"
ready-for-human = "Ready"

[vocabulary.resolution]
wontfix = "Won't Do"

[vocabulary.type]
bug = "bug"
enhancement = "story"

[workflow]
initial = "Triage"

[[workflow.statuses]]
name = "Triage"
category = "todo"

[[workflow.statuses]]
name = "Needs Info"
category = "todo"

[[workflow.statuses]]
name = "Ready"
category = "todo"
ready = true

[[workflow.statuses]]
name = "In Progress"
category = "active"

[[workflow.statuses]]
name = "Done"
category = "done"

[[workflow.statuses]]
name = "Cancelled"
category = "canceled"

# The five triage roles are transitions named after the skill's label
# vocabulary, so `orbit transition <id> <role>` round-trips each one.
[[workflow.transitions]]
name = "needs-triage"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Triage"

[[workflow.transitions]]
name = "needs-info"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Needs Info"

[[workflow.transitions]]
name = "ready-for-agent"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Ready"

[[workflow.transitions]]
name = "ready-for-human"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Ready"

[[workflow.transitions]]
name = "wontfix"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Cancelled"
resolution = "Won't Do"

[[workflow.transitions]]
name = "claim"
from = ["Ready", "In Progress"]
to = "In Progress"

[[workflow.transitions]]
name = "release"
from = ["In Progress"]
to = "Ready"

[[workflow.transitions]]
name = "done"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Done"
resolution = "completed"

[[workflow.transitions]]
name = "cancel"
from = ["Triage", "Needs Info", "Ready", "In Progress"]
to = "Cancelled"
resolution = "cancelled"

[[workflow.transitions]]
name = "reopen"
from = ["Done", "Cancelled"]
to = "Triage"
"#;

/// The `[[profiles]]` record `orbit setup <plugin>` writes into
/// `config.toml`. It captures everything doctor and capabilities need to
/// re-negotiate the profile's requirements without re-reading the profile
/// file, which may have moved or changed since it was applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedProfile {
    pub name: String,
    /// Where the profile came from: `builtin`, `board`, `orbit`, `user`,
    /// `project`, `local`, or `file:<path>` for an explicit path argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The minimum contract version the profile was written for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional: Vec<String>,
    /// Hold names the profile contributed to `board.non_ready_labels`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<String>,
    /// Role name → assignee kind (`agent` or `human`) the plugin uses.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, String>,
    /// Plugin-term → Orbit-term maps, one string→string table per
    /// vocabulary area (`status`, `label`, `priority`, `type`, `link`,
    /// `resolution`, `component`, or a plugin-defined area).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vocabulary: BTreeMap<String, BTreeMap<String, String>>,
}

/// The optional `[board]` overrides a profile may carry. Every field is
/// optional; an absent field leaves the Board's current value alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileBoard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priorities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_ready_labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<IssueTypeConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<Vec<HierarchyLevelConfig>>,
}

/// A parsed profile file. `name` defaults to the file stem when the
/// document does not set it.
#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Minimum contract version the profile requires, as semver.
    #[serde(default)]
    pub contract: Option<String>,
    /// Capabilities this binary must provide for the profile to work.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Capabilities the profile uses when present but tolerates missing.
    #[serde(default)]
    pub optional: Vec<String>,
    /// Hold names merged into `board.non_ready_labels`.
    #[serde(default)]
    pub holds: Vec<String>,
    /// Plugin role name → assignee kind (`agent` or `human`).
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    /// Plugin-term → Orbit-term maps; recorded verbatim for consumers.
    #[serde(default)]
    pub vocabulary: BTreeMap<String, BTreeMap<String, String>>,
    /// The workflow the profile selects: a shipped template name or a
    /// custom `[workflow]` definition, same shape as `config.toml`.
    #[serde(default)]
    pub workflow: Option<WorkflowConfig>,
    /// `[policy]` rules the profile contributes: merged into the Board's
    /// `config.policy.rules` by name on `orbit setup`. Rules may scope
    /// themselves to this profile through `profiles = ["<name>"]`.
    #[serde(default)]
    pub policy: crate::policy::PolicyConfig,
    /// Optional `board.*` overrides applied with the profile.
    #[serde(default)]
    pub board: ProfileBoard,
}

/// A profile plus where it was resolved from.
#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub profile: Profile,
    /// `builtin`, `board`, `orbit`, `user`, `project`, `local`, or
    /// `file:<path>` for an explicit path argument.
    pub source: String,
    /// The file it was read from, when it came from disk.
    pub path: Option<PathBuf>,
}

/// What a profile needs that this binary does not provide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Unmet {
    /// `requires` entries that are not in [`crate::capabilities::SUPPORTED`].
    pub capabilities: Vec<String>,
    /// The profile's minimum contract when it is newer than this binary's.
    pub contract: Option<String>,
}

impl Unmet {
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty() && self.contract.is_none()
    }

    /// One-line-per-item description for errors and doctor output.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for capability in &self.capabilities {
            lines.push(format!("capability `{capability}`"));
        }
        if let Some(required) = &self.contract {
            lines.push(format!(
                "contract {required} (this orbit speaks {})",
                contract::VERSION
            ));
        }
        lines
    }
}

impl Profile {
    /// Parses and validates a profile document. `fallback_name` supplies
    /// the name when the document omits it (normally the file stem).
    pub fn parse(text: &str, fallback_name: &str) -> Result<Self> {
        let mut profile: Profile = toml::from_str(text).map_err(|error| {
            OrbitError::config(format!("invalid profile `{fallback_name}`: {error}"))
        })?;
        if profile.name.is_none() {
            profile.name = Some(fallback_name.to_string());
        }
        profile.validate()?;
        Ok(profile)
    }

    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or("")
    }

    /// Syntactic and semantic validation: a usable name, well-formed
    /// capability and role entries, a parseable minimum contract, valid
    /// hold labels, and a workflow that resolves on this binary.
    pub fn validate(&self) -> Result<()> {
        let name = self.name();
        if !task::is_valid_label(name) {
            return Err(OrbitError::config(format!(
                "profile name `{name}` must be lowercase letters, digits, `-`, or `:`"
            )));
        }
        for capability in self.requires.iter().chain(self.optional.iter()) {
            if capability.trim().is_empty() {
                return Err(OrbitError::config(format!(
                    "profile `{name}` has an empty capability name"
                )));
            }
        }
        for (role, kind) in &self.roles {
            if !task::is_valid_label(role) {
                return Err(OrbitError::config(format!(
                    "profile `{name}` has invalid role name `{role}`"
                )));
            }
            if kind != "agent" && kind != "human" {
                return Err(OrbitError::config(format!(
                    "profile `{name}` role `{role}` must map to `agent` or `human`, not `{kind}`"
                )));
            }
        }
        for hold in &self.holds {
            if !task::is_valid_label(hold) {
                return Err(OrbitError::config(format!(
                    "profile `{name}` has invalid hold `{hold}`"
                )));
            }
        }
        if let Some(contract) = &self.contract {
            contract::Semver::parse(contract).map_err(|_| {
                OrbitError::config(format!(
                    "profile `{name}` contract `{contract}` is not semver (expected major.minor.patch)"
                ))
            })?;
        }
        if let Some(workflow) = &self.workflow {
            // Reuse the kernel's own workflow validation by resolving the
            // section against an otherwise-default Config.
            let config = Config {
                workflow: workflow.clone(),
                ..Config::default()
            };
            Workflow::from_config(&config)?;
        }
        // Policy rules get their shape checks here; references to
        // transitions, statuses, and profiles are validated when the merged
        // config is saved (`Config::validate`).
        let mut rule_names = std::collections::BTreeSet::new();
        for rule in &self.policy.rules {
            rule.validate_shape()?;
            if !rule_names.insert(rule.name.as_str()) {
                return Err(OrbitError::config(format!(
                    "profile `{name}` defines policy rule `{}` more than once",
                    rule.name
                )));
            }
        }
        Ok(())
    }

    /// Requirements this binary does not meet: `requires` capabilities
    /// outside [`crate::capabilities::SUPPORTED`] and a minimum contract
    /// newer than [`contract::VERSION`]. `optional` entries never block.
    pub fn unmet(&self) -> Unmet {
        unmet_parts(&self.requires, self.contract.as_deref())
    }

    /// The `[[profiles]]` record written into `config.toml`.
    pub fn to_applied(&self, source: Option<&str>) -> AppliedProfile {
        AppliedProfile {
            name: self.name().to_string(),
            source: source.map(str::to_string),
            description: self.description.clone(),
            contract: self.contract.clone(),
            requires: self.requires.clone(),
            optional: self.optional.clone(),
            holds: self.holds.clone(),
            roles: self.roles.clone(),
            vocabulary: self.vocabulary.clone(),
        }
    }

    /// Applies the profile to a Board config: workflow and `board.*`
    /// overrides, hold labels merged into `non_ready_labels`, and the
    /// `[[profiles]]` record upserted by name. Re-applying the same
    /// profile is a no-op.
    pub fn apply(&self, config: &mut Config, source: Option<&str>) -> Result<()> {
        if let Some(workflow) = &self.workflow {
            config.workflow = workflow.clone();
        }
        // Profile policy rules merge by name: re-applying replaces the
        // profile's own rules and leaves hand-written rules alone.
        for rule in &self.policy.rules {
            if let Some(existing) = config
                .policy
                .rules
                .iter_mut()
                .find(|existing| existing.name == rule.name)
            {
                *existing = rule.clone();
            } else {
                config.policy.rules.push(rule.clone());
            }
        }
        let board = &self.board;
        if let Some(priorities) = &board.priorities {
            config.board.priorities = priorities.clone();
        }
        if let Some(default_priority) = &board.default_priority {
            config.board.default_priority = default_priority.clone();
        }
        if let Some(non_ready_labels) = &board.non_ready_labels {
            config.board.non_ready_labels = non_ready_labels.clone();
        }
        if let Some(components) = &board.components {
            config.board.components = components.clone();
        }
        if let Some(default_type) = &board.default_type {
            config.board.default_type = default_type.clone();
        }
        if let Some(types) = &board.types {
            config.board.types = types.clone();
        }
        if let Some(hierarchy) = &board.hierarchy {
            config.board.hierarchy = hierarchy.clone();
        }
        for hold in &self.holds {
            if !config.board.non_ready_labels.contains(hold) {
                config.board.non_ready_labels.push(hold.clone());
            }
        }
        let applied = self.to_applied(source);
        if let Some(existing) = config
            .profiles
            .iter_mut()
            .find(|entry| entry.name == applied.name)
        {
            *existing = applied;
        } else {
            config.profiles.push(applied);
        }
        Ok(())
    }
}

/// The shared requirement check behind [`Profile::unmet`] and
/// [`unmet_applied`]: `requires` names outside the supported set plus a
/// minimum contract newer than this binary's.
fn unmet_parts(requires: &[String], contract_version: Option<&str>) -> Unmet {
    let mut unmet = Unmet::default();
    for capability in requires {
        if !crate::capabilities::is_supported(capability) {
            unmet.capabilities.push(capability.clone());
        }
    }
    if let Some(required) = contract_version
        && let Ok(required) = contract::Semver::parse(required)
        && required > contract::binary_semver()
    {
        unmet.contract = contract_version.map(str::to_string);
    }
    unmet
}

/// Finds a profile by name. Resolution order:
///
/// 1. An explicit path (`orbit setup ./acme.toml` or any name containing
///    `/` or ending in `.toml`) relative to the current directory.
/// 2. Profiles shipped inside the binary.
/// 3. `<board>/profiles/<name>.toml` — inside the Board's own repository.
/// 4. `<orbit>/profiles/<name>.toml` — beside the Board folder in the
///    Orbit folder.
/// 5. `<project>/profiles/<name>.toml` — the enclosing Git repository.
/// 6. `~/.config/orbit/profiles/<name>.toml` — the user-level profile.
/// 7. `./profiles/<name>.toml` — the current directory.
pub fn find(name: &str, dir: Option<&Path>) -> Result<Option<ResolvedProfile>> {
    if name.contains('/') || name.ends_with(".toml") {
        let path = resolve_path_arg(name)?;
        let Some(path) = path else {
            return Err(OrbitError::new(
                code::NOT_FOUND,
                format!("profile file `{name}` does not exist"),
            ));
        };
        let profile = load_file(&path)?;
        return Ok(Some(ResolvedProfile {
            profile,
            source: format!("file:{name}"),
            path: Some(path),
        }));
    }

    for (builtin, text) in BUILTIN_PROFILES {
        if *builtin == name {
            let profile = Profile::parse(text, name)?;
            return Ok(Some(ResolvedProfile {
                profile,
                source: "builtin".to_string(),
                path: None,
            }));
        }
    }

    for (directory, source) in search_dirs(dir) {
        let candidate = directory.join(format!("{name}.toml"));
        if candidate.is_file() {
            let profile = load_file(&candidate)?;
            return Ok(Some(ResolvedProfile {
                profile,
                source: source.to_string(),
                path: Some(candidate),
            }));
        }
    }
    Ok(None)
}

/// A profile shipped inside the binary, parsed on demand. Returns `None`
/// for names that are not built in. Built-in TOML is a compile-time
/// constant, so a parse failure is a build bug and panics.
pub fn builtin(name: &str) -> Option<Profile> {
    BUILTIN_PROFILES
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, text)| {
            Profile::parse(text, name).expect("built-in profile must parse and validate")
        })
}

fn resolve_path_arg(name: &str) -> Result<Option<PathBuf>> {
    let path = if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        std::env::current_dir()
            .map_err(|error| OrbitError::io("resolve current directory", error))?
            .join(name)
    };
    Ok(path.is_file().then_some(path))
}

/// `(directory, source-label)` pairs searched for `<name>.toml`, in
/// priority order, deduplicated by directory.
fn search_dirs(dir: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut dirs: Vec<(PathBuf, &'static str)> = Vec::new();
    let mut push = |path: PathBuf, source: &'static str| {
        if !dirs.iter().any(|(seen, _)| *seen == path) {
            dirs.push((path, source));
        }
    };

    // Board-local first: `<board>/profiles` and `<orbit>/profiles`.
    let mut discovery = Board::discovery(dir);
    if let Some(root) = discovery.root.take() {
        push(root.join("profiles"), "board");
    }
    if let Some(orbit_dir) = discovery.orbit_dir.take() {
        push(orbit_dir.join("profiles"), "orbit");
    }

    // The enclosing Git repository, then the user-level profile home.
    if let Ok(root) = crate::git::repo_root() {
        push(root.join("profiles"), "project");
    }
    if let Some(home) = crate::git::home_dir() {
        push(home.join(".config").join("orbit").join("profiles"), "user");
    }
    if let Ok(cwd) = std::env::current_dir() {
        push(cwd.join("profiles"), "local");
    }
    dirs
}

fn load_file(path: &Path) -> Result<Profile> {
    let content = std::fs::read_to_string(path)
        .map_err(|error| OrbitError::io(format!("read {}", path.display()), error))?;
    let fallback = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("profile");
    Profile::parse(&content, fallback)
}

/// Re-checks an applied `[[profiles]]` record against this binary —
/// the same negotiation `orbit setup` performs before writing.
pub fn unmet_applied(applied: &AppliedProfile) -> Unmet {
    unmet_parts(&applied.requires, applied.contract.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_profile() {
        let profile = Profile::parse(
            r#"
name = "acme"
description = "Acme plugin"
contract = "1.0.0"
requires = ["workflow", "links"]
optional = ["remote"]
holds = ["needs-info"]

[roles]
reviewer = "human"

[vocabulary.status]
triage = "in_review"

[workflow]
template = "agentic"

[board]
default_priority = "high"
"#,
            "acme",
        )
        .unwrap();
        assert_eq!(profile.name(), "acme");
        assert_eq!(profile.requires, ["workflow", "links"]);
        assert_eq!(profile.roles["reviewer"], "human");
        assert_eq!(profile.vocabulary["status"]["triage"], "in_review");
        assert_eq!(
            profile.workflow.as_ref().unwrap().template.as_deref(),
            Some("agentic")
        );
        assert!(profile.unmet().is_empty());
    }

    #[test]
    fn name_defaults_to_the_file_stem() {
        let profile = Profile::parse("requires = []\n", "kinetic").unwrap();
        assert_eq!(profile.name(), "kinetic");
    }

    #[test]
    fn rejects_bad_names_roles_and_contracts() {
        assert!(Profile::parse("name = \"Bad Name\"\n", "x").is_err());
        assert!(Profile::parse("name = \"a\"\n[roles]\nr = \"robot\"\n", "a").is_err());
        assert!(Profile::parse("name = \"a\"\ncontract = \"v1\"\n", "a").is_err());
        assert!(Profile::parse("name = \"a\"\nholds = [\"Bad Hold\"]\n", "a").is_err());
        assert!(Profile::parse("name = \"a\"\n[workflow]\ntemplate = \"nope\"\n", "a").is_err());
    }

    #[test]
    fn unmet_lists_missing_capabilities_and_contract() {
        let profile = Profile::parse(
            "name = \"a\"\nrequires = [\"workflow\", \"teleport\"]\ncontract = \"99.0.0\"\n",
            "a",
        )
        .unwrap();
        let unmet = profile.unmet();
        assert_eq!(unmet.capabilities, ["teleport"]);
        assert_eq!(unmet.contract.as_deref(), Some("99.0.0"));
        assert!(!unmet.describe().is_empty());
    }

    #[test]
    fn apply_is_idempotent() {
        let profile = Profile::parse(
            r#"
name = "acme"
holds = ["needs-info"]
[workflow]
template = "minimal"
[board]
components = ["api"]
"#,
            "acme",
        )
        .unwrap();
        let mut config = Config::default();
        profile.apply(&mut config, Some("user")).unwrap();
        let once = config.to_toml().unwrap();
        profile.apply(&mut config, Some("user")).unwrap();
        let twice = config.to_toml().unwrap();
        assert_eq!(once, twice);
        assert_eq!(config.workflow.template.as_deref(), Some("minimal"));
        assert!(
            config
                .board
                .non_ready_labels
                .contains(&"needs-info".to_string())
        );
        assert_eq!(config.board.components, ["api"]);
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].name, "acme");
        assert_eq!(config.profiles[0].source.as_deref(), Some("user"));
        // The serialized config must round-trip: `[[profiles]]` sits among
        // the table sections without swallowing `contract`.
        let reparsed: Config = toml::from_str(&twice).unwrap();
        assert_eq!(reparsed.profiles.len(), 1);
        assert_eq!(reparsed.contract, config.contract);
        config.validate().unwrap();
    }

    #[test]
    fn unmet_applied_matches_profile_unmet() {
        let applied = AppliedProfile {
            name: "a".to_string(),
            source: None,
            description: None,
            contract: Some("99.0.0".to_string()),
            requires: vec!["workflow".to_string(), "teleport".to_string()],
            optional: Vec::new(),
            holds: Vec::new(),
            roles: BTreeMap::new(),
            vocabulary: BTreeMap::new(),
        };
        let unmet = unmet_applied(&applied);
        assert_eq!(unmet.capabilities, ["teleport"]);
        assert!(unmet.contract.is_some());
    }
}
