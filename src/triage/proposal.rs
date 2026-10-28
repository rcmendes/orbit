use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{OrbitError, Result, code};
use crate::github::fields::{parse_area, validate_verification};
use crate::task::{Task, normalize_label, normalize_verification};
use crate::triage::labels::{validate_category_roles, validate_label_list, validate_state_roles};

const FORBIDDEN_FIELDS: &[&str] = &["title", "body", "status", "state"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageProposal {
    #[serde(default)]
    pub rationale: Option<String>,
    #[serde(default)]
    pub uncertainty: Option<String>,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub clear_area: bool,
    #[serde(default)]
    pub verification: Option<String>,
    #[serde(default)]
    pub clear_verification: bool,
    #[serde(default)]
    pub labels_add: Vec<String>,
    #[serde(default)]
    pub labels_remove: Vec<String>,
}

impl TriageProposal {
    pub fn parse_json(raw: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(raw).map_err(|error| {
            OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                format!("proposal is not valid JSON: {error}"),
            )
        })?;
        reject_forbidden_fields(&value)?;
        serde_json::from_value(value).map_err(|error| {
            OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                format!("proposal schema is invalid: {error}"),
            )
        })
    }

    pub fn normalize(&mut self) -> Result<()> {
        if self.clear_area && self.area.is_some() {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                "proposal cannot set both area and clear_area",
            ));
        }
        if self.clear_verification && self.verification.is_some() {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                "proposal cannot set both verification and clear_verification",
            ));
        }

        let mut add = Vec::new();
        for label in &self.labels_add {
            add.push(normalize_label(label)?);
        }
        let mut remove = Vec::new();
        for label in &self.labels_remove {
            remove.push(normalize_label(label)?);
        }
        add.sort();
        add.dedup();
        remove.sort();
        remove.dedup();
        validate_label_list(&add, "labels_add")?;
        validate_label_list(&remove, "labels_remove")?;
        self.labels_add = add;
        self.labels_remove = remove;

        if let Some(area) = self.area.take() {
            let parsed = parse_area(&area).map_err(|_| {
                let allowed = crate::task::TaskArea::all()
                    .iter()
                    .map(|area| area.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                OrbitError::new(
                    code::INVALID_AREA,
                    format!("unknown area `{area}`; expected one of: {allowed}"),
                )
            })?;
            self.area = Some(parsed.as_str().to_string());
        }

        if let Some(verification) = self.verification.take() {
            let normalized = normalize_verification(Some(verification))?;
            if let Some(value) = normalized {
                validate_verification(&value)?;
                self.verification = Some(value);
            }
        }

        Ok(())
    }

    pub fn has_metadata_changes(&self) -> bool {
        self.area.is_some()
            || self.clear_area
            || self.verification.is_some()
            || self.clear_verification
            || !self.labels_add.is_empty()
            || !self.labels_remove.is_empty()
    }

    pub fn build_preview(&self, task: &Task) -> Result<(Value, bool)> {
        if self.uncertainty.is_some() && !self.has_metadata_changes() {
            return Ok((json!({}), false));
        }
        if !self.has_metadata_changes() {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                "proposal makes no metadata changes; add fields to change or explain uncertainty without applying",
            ));
        }

        let mut changes = serde_json::Map::new();

        if self.clear_area {
            if task.front.area.is_some() {
                changes.insert(
                    "area".into(),
                    json!({
                        "from": task.front.area.map(|area| area.as_str()),
                        "to": null
                    }),
                );
            }
        } else if let Some(area) = &self.area {
            let current = task.front.area.map(|area| area.as_str());
            if current != Some(area.as_str()) {
                changes.insert("area".into(), json!({ "from": current, "to": area }));
            }
        }

        if self.clear_verification {
            if task.front.verification.is_some() {
                changes.insert(
                    "verification".into(),
                    json!({
                        "from": task.front.verification,
                        "to": null
                    }),
                );
            }
        } else if let Some(verification) = &self.verification
            && task.front.verification.as_deref() != Some(verification.as_str())
        {
            changes.insert(
                "verification".into(),
                json!({
                    "from": task.front.verification,
                    "to": verification
                }),
            );
        }

        if !self.labels_add.is_empty() || !self.labels_remove.is_empty() {
            let resulting =
                labels_after_proposal(&task.front.labels, &self.labels_add, &self.labels_remove);
            validate_state_roles(&resulting)?;
            validate_category_roles(&resulting)?;
        }

        let mut add = Vec::new();
        let mut remove = Vec::new();
        for label in &self.labels_add {
            if !task.front.labels.contains(label) {
                add.push(label.clone());
            }
        }
        for label in &self.labels_remove {
            if task.front.labels.contains(label) {
                remove.push(label.clone());
            }
        }
        if !add.is_empty() || !remove.is_empty() {
            changes.insert("labels".into(), json!({ "add": add, "remove": remove }));
        }

        Ok((Value::Object(changes), true))
    }

    pub fn apply_to(&self, task: &mut Task) -> Result<bool> {
        let (changes, _) = self.build_preview(task)?;
        if changes.as_object().is_some_and(|map| map.is_empty()) {
            return Ok(false);
        }

        if self.clear_area {
            task.front.area = None;
        } else if let Some(area) = &self.area {
            task.front.area = Some(parse_area(area)?);
        }

        if self.clear_verification {
            task.front.verification = None;
        } else if let Some(verification) = &self.verification {
            task.front.verification = Some(verification.clone());
        }

        if !self.labels_add.is_empty() || !self.labels_remove.is_empty() {
            task.front.labels =
                labels_after_proposal(&task.front.labels, &self.labels_add, &self.labels_remove);
        }

        Ok(true)
    }
}

fn labels_after_proposal(current: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    let mut labels = current.to_vec();
    for label in add {
        if !labels.contains(label) {
            labels.push(label.clone());
        }
    }
    labels.retain(|label| !remove.contains(label));
    labels.sort();
    labels.dedup();
    labels
}

fn reject_forbidden_fields(value: &Value) -> Result<()> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for field in FORBIDDEN_FIELDS {
        if object.contains_key(*field) {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                format!(
                    "proposal must not include GitHub-owned field `{field}`; triage only updates area, verification, and canonical labels on Orbit"
                ),
            ));
        }
    }
    Ok(())
}
