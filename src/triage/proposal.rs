use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{OrbitError, Result, code};
use crate::task::{Task, normalize_component, normalize_label, normalize_verification};
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
    pub component: Option<String>,
    #[serde(default)]
    pub clear_component: bool,
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

    pub fn normalize(&mut self, allowed_components: &[String]) -> Result<()> {
        if self.clear_component && self.component.is_some() {
            return Err(OrbitError::new(
                code::TRIAGE_INVALID_PROPOSAL,
                "proposal cannot set both component and clear_component",
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

        if let Some(component) = self.component.take() {
            self.component = Some(normalize_component(&component, allowed_components)?);
        }

        if let Some(verification) = self.verification.take() {
            self.verification = normalize_verification(Some(verification))?;
        }

        Ok(())
    }

    pub fn has_metadata_changes(&self) -> bool {
        self.component.is_some()
            || self.clear_component
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

        if self.clear_component {
            if task.front.component.is_some() {
                changes.insert(
                    "component".into(),
                    json!({
                        "from": task.front.component.as_deref(),
                        "to": null
                    }),
                );
            }
        } else if let Some(component) = &self.component {
            let current = task.front.component.as_deref();
            if current != Some(component.as_str()) {
                changes.insert(
                    "component".into(),
                    json!({ "from": current, "to": component }),
                );
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

        if self.clear_component {
            task.front.component = None;
        } else if let Some(component) = &self.component {
            task.front.component = Some(component.clone());
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
                    "proposal must not include non-Triage Task field `{field}`; triage only updates component, verification, and canonical labels on Orbit"
                ),
            ));
        }
    }
    Ok(())
}
