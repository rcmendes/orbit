use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;

#[derive(Debug, Default, Serialize)]
pub struct MigrationReport {
    pub migrated: bool,
    pub resumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    pub tasks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pointer: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<PathBuf>,
}

impl MigrationReport {
    pub fn render_human(&self) -> String {
        if !self.migrated {
            return self
                .reason
                .as_ref()
                .map(|reason| format!("migration: {reason}\n"))
                .unwrap_or_default();
        }
        let from = self
            .from
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "legacy layout".to_string());
        let to = self
            .to
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        format!(
            "migrated Board from {from} to {to} ({} tasks{})\n",
            self.tasks,
            if self.resumed { ", resumed" } else { "" }
        )
    }
}

pub fn migrate(_dir: Option<&Path>) -> Result<MigrationReport> {
    Ok(MigrationReport {
        migrated: false,
        reason: Some("no legacy layout to migrate".to_string()),
        ..MigrationReport::default()
    })
}
