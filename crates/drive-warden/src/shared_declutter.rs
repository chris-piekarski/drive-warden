use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use gdrive_core::{DriveGateway, InventoryRepository, GOOGLE_DRIVE_FOLDER_MIME};
use serde::Serialize;

use crate::shared_backup::backed_up_ids_from_manifest;

#[derive(Debug, Clone)]
pub struct SharedDeclutterOptions {
    pub manifest_path: PathBuf,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedDeclutterPlan {
    pub manifest: String,
    pub total_shared_with_me: usize,
    pub actionable_count: usize,
    pub backed_up_count: usize,
    pub folder_placeholder_count: usize,
    pub unresolved_count: usize,
    pub not_in_manifest_count: usize,
    pub entries: Vec<SharedDeclutterEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedDeclutterEntry {
    pub id: String,
    pub name: String,
    pub path: String,
    pub mime_type: String,
    pub classification: String,
    pub actionable: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedDeclutterApplySummary {
    pub attempted: usize,
    pub removed: usize,
    pub failed: usize,
    pub failures: Vec<SharedDeclutterFailure>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedDeclutterFailure {
    pub id: String,
    pub name: String,
    pub error: String,
}

pub fn plan_shared_declutter(
    repository: &dyn InventoryRepository,
    options: &SharedDeclutterOptions,
) -> Result<SharedDeclutterPlan> {
    let backed_up = backed_up_ids_from_manifest(&options.manifest_path)?;
    let mut items = repository
        .load_inventory_items()
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .filter(|item| item.file.shared && !item.file.owned_by_me && !item.file.trashed)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.path
            .primary_path
            .cmp(&right.path.primary_path)
            .then_with(|| left.file.name.cmp(&right.file.name))
            .then_with(|| left.file.id.cmp(&right.file.id))
    });
    if let Some(limit) = options.limit {
        items.truncate(limit);
    }

    let entries = items.iter().map(|item| classify_item(item, &backed_up)).collect::<Vec<_>>();
    let actionable_count = entries.iter().filter(|entry| entry.actionable).count();
    let backed_up_count =
        entries.iter().filter(|entry| entry.classification == "backed_up").count();
    let folder_placeholder_count =
        entries.iter().filter(|entry| entry.classification == "folder_placeholder").count();
    let unresolved_count =
        entries.iter().filter(|entry| entry.classification == "unresolved").count();
    let not_in_manifest_count =
        entries.iter().filter(|entry| entry.classification == "not_in_manifest").count();

    Ok(SharedDeclutterPlan {
        manifest: options.manifest_path.display().to_string(),
        total_shared_with_me: entries.len(),
        actionable_count,
        backed_up_count,
        folder_placeholder_count,
        unresolved_count,
        not_in_manifest_count,
        entries,
    })
}

pub async fn apply_shared_declutter(
    gateway: &dyn DriveGateway,
    plan: &SharedDeclutterPlan,
) -> Result<SharedDeclutterApplySummary> {
    let actionable = plan.entries.iter().filter(|entry| entry.actionable).collect::<Vec<_>>();
    let mut failures = Vec::new();
    let mut removed = 0usize;
    for entry in &actionable {
        match gateway.remove_my_drive_parent(&entry.id).await {
            Ok(_) => removed += 1,
            Err(error) => failures.push(SharedDeclutterFailure {
                id: entry.id.clone(),
                name: entry.name.clone(),
                error: error.to_string(),
            }),
        }
    }
    Ok(SharedDeclutterApplySummary {
        attempted: actionable.len(),
        removed,
        failed: failures.len(),
        failures,
    })
}

fn classify_item(
    item: &gdrive_core::InventoryItem,
    backed_up: &BTreeSet<String>,
) -> SharedDeclutterEntry {
    let (classification, actionable, reason) = if !backed_up.contains(&item.file.id) {
        (
            "not_in_manifest",
            false,
            "no successful shared backup manifest record exists for this item",
        )
    } else if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
        (
            "folder_placeholder",
            false,
            "folder backup records are placeholders; verify descendant coverage before removing folder shortcuts",
        )
    } else if item.file.mime_type == "application/vnd.google-apps.earth" {
        (
            "unresolved",
            false,
            "Google Earth projects require manual browser export before declutter",
        )
    } else {
        ("backed_up", true, "successful backup manifest record exists")
    };
    SharedDeclutterEntry {
        id: item.file.id.clone(),
        name: item.file.name.clone(),
        path: item.path.primary_path.clone(),
        mime_type: item.file.mime_type.clone(),
        classification: classification.into(),
        actionable,
        reason: reason.into(),
    }
}
