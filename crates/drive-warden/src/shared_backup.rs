use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use gdrive_core::{DriveGateway, InventoryItem, InventoryRepository, GOOGLE_DRIVE_FOLDER_MIME};
use serde::{Deserialize, Serialize};

const GOOGLE_DOC_MIME: &str = "application/vnd.google-apps.document";
const GOOGLE_SHEET_MIME: &str = "application/vnd.google-apps.spreadsheet";
const GOOGLE_SLIDES_MIME: &str = "application/vnd.google-apps.presentation";
const GOOGLE_DRAWING_MIME: &str = "application/vnd.google-apps.drawing";
const GOOGLE_SCRIPT_MIME: &str = "application/vnd.google-apps.script";
const GOOGLE_MAP_MIME: &str = "application/vnd.google-apps.map";
const GOOGLE_EARTH_MIME: &str = "application/vnd.google-apps.earth";

#[derive(Debug, Clone)]
pub struct SharedBackupOptions {
    pub out_dir: PathBuf,
    pub manifest_path: Option<PathBuf>,
    pub reuse_manifest: Option<PathBuf>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedBackupRecord {
    pub id: String,
    pub name: String,
    pub primary_path: String,
    pub mime_type: String,
    pub owner: String,
    pub status: String,
    pub local_path: Option<String>,
    pub local_size: Option<u64>,
    pub source_modified_time: Option<String>,
    pub web_view_link: Option<String>,
    pub export_mime_type: Option<String>,
    pub recovery_method: Option<String>,
    pub reason: Option<String>,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SharedBackupSummary {
    pub backup_dir: String,
    pub manifest: String,
    pub total_shared_with_me: usize,
    pub completed: usize,
    pub unresolved: usize,
    pub local_file_bytes: u64,
    pub counts: BTreeMap<String, usize>,
    pub unresolved_records: Vec<SharedBackupRecord>,
}

#[derive(Debug, Clone)]
struct ExportAttempt {
    label: &'static str,
    mime_type: &'static str,
    extension: &'static str,
    direct_url: Option<String>,
    recovery_method: Option<&'static str>,
}

pub async fn backup_shared_with_me(
    gateway: &dyn DriveGateway,
    repository: &dyn InventoryRepository,
    options: &SharedBackupOptions,
) -> Result<SharedBackupSummary> {
    let manifest_path =
        options.manifest_path.clone().unwrap_or_else(|| options.out_dir.join("manifest.jsonl"));
    let mut completed = load_completed_records(&manifest_path)?;
    let reuse_files = load_reuse_files(options.reuse_manifest.as_deref())?;
    let mut items = repository
        .load_inventory_items()
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .filter(|item| item.file.shared && !item.file.owned_by_me && !item.file.trashed)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.path
            .depth
            .cmp(&right.path.depth)
            .then_with(|| left.path.primary_path.cmp(&right.path.primary_path))
            .then_with(|| left.file.name.cmp(&right.file.name))
    });
    if let Some(limit) = options.limit {
        items.truncate(limit);
    }

    for item in &items {
        if completed.contains_key(&item.file.id) {
            continue;
        }
        let record = backup_item(gateway, item, options, &reuse_files).await?;
        append_manifest_record(&manifest_path, &record)?;
        if is_success_status(&record.status) {
            completed.insert(record.id.clone(), record);
        }
    }

    build_summary(&options.out_dir, &manifest_path, &items, &completed)
}

pub fn backed_up_ids_from_manifest(path: &Path) -> Result<BTreeSet<String>> {
    Ok(load_completed_records(path)?.into_keys().collect())
}

pub fn unresolved_records_from_manifest(path: &Path) -> Result<Vec<SharedBackupRecord>> {
    Ok(load_all_records(path)?
        .into_iter()
        .filter(|record| !is_success_status(&record.status))
        .collect())
}

async fn backup_item(
    gateway: &dyn DriveGateway,
    item: &InventoryItem,
    options: &SharedBackupOptions,
    reuse_files: &ReuseFiles,
) -> Result<SharedBackupRecord> {
    let base = base_record(item);
    if item.file.mime_type == GOOGLE_DRIVE_FOLDER_MIME {
        let target = output_path(&options.out_dir, item, None);
        fs::create_dir_all(&target)
            .with_context(|| format!("failed to create backup folder `{}`", target.display()))?;
        return Ok(SharedBackupRecord {
            status: "folder".into(),
            local_path: Some(target.display().to_string()),
            local_size: None,
            ..base
        });
    }

    if let Some(source) = reuse_files.find(item) {
        let target =
            output_path(&options.out_dir, item, source.extension().and_then(|ext| ext.to_str()));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, &target).with_context(|| {
            format!(
                "failed to copy reusable backup `{}` to `{}`",
                source.display(),
                target.display()
            )
        })?;
        return Ok(SharedBackupRecord {
            status: "copied".into(),
            local_path: Some(target.display().to_string()),
            local_size: Some(target.metadata()?.len()),
            ..base
        });
    }

    if let Some(attempts) = export_attempts(item) {
        let mut errors = Vec::new();
        for attempt in attempts {
            let target = output_path(&options.out_dir, item, Some(attempt.extension));
            let bytes = match &attempt.direct_url {
                Some(url) => gateway.download_url(url).await,
                None => gateway.export_file(&item.file.id, attempt.mime_type).await,
            };
            match bytes {
                Ok(bytes) => {
                    write_bytes(&target, &bytes)?;
                    return Ok(SharedBackupRecord {
                        status: attempt
                            .recovery_method
                            .map(|_| "recovered_export")
                            .unwrap_or("exported")
                            .into(),
                        local_path: Some(target.display().to_string()),
                        local_size: Some(bytes.len() as u64),
                        export_mime_type: Some(attempt.mime_type.into()),
                        recovery_method: attempt.recovery_method.map(str::to_string),
                        ..base
                    });
                }
                Err(error) => errors.push(format!("{}: {}", attempt.label, error)),
            }
        }
        return Ok(SharedBackupRecord {
            status: "error".into(),
            reason: Some(errors.join(" | ")),
            ..base
        });
    }

    if item.file.mime_type.starts_with("application/vnd.google-apps.") {
        return Ok(SharedBackupRecord {
            status: "skipped".into(),
            reason: Some("unsupported Google-native export; manual export may be required".into()),
            ..base
        });
    }

    let target = output_path(&options.out_dir, item, None);
    match gateway.download_file(&item.file.id).await {
        Ok(bytes) => {
            write_bytes(&target, &bytes)?;
            Ok(SharedBackupRecord {
                status: "downloaded".into(),
                local_path: Some(target.display().to_string()),
                local_size: Some(bytes.len() as u64),
                ..base
            })
        }
        Err(error) => Ok(SharedBackupRecord {
            status: "error".into(),
            reason: Some(error.to_string()),
            ..base
        }),
    }
}

fn export_attempts(item: &InventoryItem) -> Option<Vec<ExportAttempt>> {
    let id = &item.file.id;
    match item.file.mime_type.as_str() {
        GOOGLE_DOC_MIME => Some(vec![
            drive_export(
                "docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                ".docx",
            ),
            direct_export(
                "direct-docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                ".docx",
                format!("https://docs.google.com/document/d/{id}/export?format=docx"),
            ),
            drive_export("pdf", "application/pdf", ".pdf"),
            drive_export("txt", "text/plain", ".txt"),
        ]),
        GOOGLE_SHEET_MIME => Some(vec![
            drive_export(
                "xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                ".xlsx",
            ),
            direct_export(
                "direct-xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                ".xlsx",
                format!("https://docs.google.com/spreadsheets/d/{id}/export?format=xlsx"),
            ),
            drive_export("csv", "text/csv", ".csv"),
            drive_export("ods", "application/vnd.oasis.opendocument.spreadsheet", ".ods"),
        ]),
        GOOGLE_SLIDES_MIME => Some(vec![
            drive_export(
                "pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                ".pptx",
            ),
            direct_export(
                "direct-pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
                ".pptx",
                format!("https://docs.google.com/presentation/d/{id}/export/pptx"),
            ),
            drive_export("pdf", "application/pdf", ".pdf"),
            drive_export("txt", "text/plain", ".txt"),
        ]),
        GOOGLE_DRAWING_MIME => Some(vec![
            drive_export("pdf", "application/pdf", ".pdf"),
            drive_export("png", "image/png", ".png"),
        ]),
        GOOGLE_SCRIPT_MIME => Some(vec![drive_export(
            "script-json",
            "application/vnd.google-apps.script+json",
            ".json",
        )]),
        GOOGLE_MAP_MIME => Some(vec![direct_export(
            "my-maps-kml",
            "application/vnd.google-earth.kml+xml",
            ".kml",
            format!("https://www.google.com/maps/d/kml?mid={id}&forcekml=1"),
        )]),
        GOOGLE_EARTH_MIME => {
            Some(vec![drive_export("earth-kml", "application/vnd.google-earth.kml+xml", ".kml")])
        }
        _ => None,
    }
}

fn drive_export(
    label: &'static str,
    mime_type: &'static str,
    extension: &'static str,
) -> ExportAttempt {
    ExportAttempt { label, mime_type, extension, direct_url: None, recovery_method: None }
}

fn direct_export(
    label: &'static str,
    mime_type: &'static str,
    extension: &'static str,
    url: String,
) -> ExportAttempt {
    ExportAttempt {
        label,
        mime_type,
        extension,
        direct_url: Some(url),
        recovery_method: Some("direct_docs_or_maps_export"),
    }
}

fn base_record(item: &InventoryItem) -> SharedBackupRecord {
    SharedBackupRecord {
        id: item.file.id.clone(),
        name: item.file.name.clone(),
        primary_path: item.path.primary_path.clone(),
        mime_type: item.file.mime_type.clone(),
        owner: owner_label(item),
        status: "pending".into(),
        local_path: None,
        local_size: None,
        source_modified_time: item.file.modified_time.map(|time| time.to_rfc3339()),
        web_view_link: item.file.web_view_link.clone(),
        export_mime_type: None,
        recovery_method: None,
        reason: None,
        generated_at: Utc::now().to_rfc3339(),
    }
}

fn owner_label(item: &InventoryItem) -> String {
    item.file
        .permissions
        .iter()
        .find(|permission| permission.role == "owner")
        .and_then(|permission| {
            permission
                .email_address
                .clone()
                .or_else(|| permission.display_name.clone())
                .or_else(|| Some(permission.id.clone()))
        })
        .unwrap_or_else(|| "(unknown)".into())
}

fn output_path(out_dir: &Path, item: &InventoryItem, extension: Option<&str>) -> PathBuf {
    let parts = item
        .path
        .primary_path
        .trim_start_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(sanitize_component)
        .collect::<Vec<_>>();
    let mut path = if parts.is_empty() {
        out_dir.join(sanitize_component(&item.file.name))
    } else {
        parts.iter().fold(out_dir.to_path_buf(), |path, part| path.join(part))
    };
    if let Some(extension) = extension {
        path.set_extension(extension.trim_start_matches('.'));
    }
    path
}

fn sanitize_component(value: &str) -> String {
    let mut cleaned = value.replace('\0', "").trim().to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "_".into();
    }
    cleaned.chars().take(180).collect()
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_file_name(format!(
        "{}.part",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("backup")
    ));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)?;
    Ok(())
}

fn append_manifest_record(path: &Path, record: &SharedBackupRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(record)?)?;
    file.flush()?;
    Ok(())
}

fn load_all_records(path: &Path) -> Result<Vec<SharedBackupRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)?;
    Ok(contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<SharedBackupRecord>(line).ok())
        .collect())
}

fn load_completed_records(path: &Path) -> Result<BTreeMap<String, SharedBackupRecord>> {
    Ok(load_all_records(path)?
        .into_iter()
        .filter(|record| is_success_status(&record.status))
        .map(|record| (record.id.clone(), record))
        .collect())
}

fn is_success_status(status: &str) -> bool {
    matches!(status, "downloaded" | "exported" | "folder" | "copied" | "recovered_export")
}

fn build_summary(
    out_dir: &Path,
    manifest_path: &Path,
    items: &[InventoryItem],
    completed: &BTreeMap<String, SharedBackupRecord>,
) -> Result<SharedBackupSummary> {
    let all_records = load_all_records(manifest_path)?;
    let mut latest = BTreeMap::<String, SharedBackupRecord>::new();
    for record in all_records {
        latest.insert(record.id.clone(), record);
    }
    let mut counts = BTreeMap::<String, usize>::new();
    let mut unresolved_records = Vec::new();
    let mut bytes = 0u64;
    for item in items {
        match completed.get(&item.file.id) {
            Some(record) => {
                *counts.entry(record.status.clone()).or_default() += 1;
                if let Some(path) = record.local_path.as_deref() {
                    let path = Path::new(path);
                    if path.is_file() {
                        bytes += path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                    }
                }
            }
            None => {
                unresolved_records.push(latest.get(&item.file.id).cloned().unwrap_or_else(|| {
                    SharedBackupRecord {
                        status: "not_attempted".into(),
                        reason: Some("no successful backup manifest record found".into()),
                        ..base_record(item)
                    }
                }))
            }
        }
    }
    Ok(SharedBackupSummary {
        backup_dir: out_dir.display().to_string(),
        manifest: manifest_path.display().to_string(),
        total_shared_with_me: items.len(),
        completed: completed.len(),
        unresolved: unresolved_records.len(),
        local_file_bytes: bytes,
        counts,
        unresolved_records,
    })
}

#[derive(Debug, Default)]
struct ReuseFiles {
    exact: BTreeMap<(String, Option<u64>), PathBuf>,
    exported_by_stem: BTreeMap<String, PathBuf>,
}

impl ReuseFiles {
    fn find(&self, item: &InventoryItem) -> Option<&PathBuf> {
        self.exact
            .get(&(item.file.name.to_ascii_lowercase(), item.file.size))
            .or_else(|| self.exported_by_stem.get(&item.file.name.to_ascii_lowercase()))
    }
}

#[derive(Debug, Deserialize)]
struct LegacyReuseRecord {
    local_path: Option<String>,
    local_name: Option<String>,
    local_size: Option<u64>,
}

fn load_reuse_files(path: Option<&Path>) -> Result<ReuseFiles> {
    let Some(path) = path else {
        return Ok(ReuseFiles::default());
    };
    if !path.exists() {
        return Ok(ReuseFiles::default());
    }
    let records = serde_json::from_str::<Vec<LegacyReuseRecord>>(&fs::read_to_string(path)?)?;
    let mut reuse = ReuseFiles::default();
    for record in records {
        let (Some(local_path), Some(local_name)) = (record.local_path, record.local_name) else {
            continue;
        };
        let path = PathBuf::from(local_path);
        if !path.exists() {
            continue;
        }
        reuse.exact.insert((local_name.to_ascii_lowercase(), record.local_size), path.clone());
        if let Some(stem) = local_name.strip_suffix(".pdf") {
            reuse.exported_by_stem.insert(stem.to_ascii_lowercase(), path);
        }
    }
    Ok(reuse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_helpers_return_successful_and_unresolved_records() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        let manifest = temp_dir.path().join("manifest.jsonl");
        let success = SharedBackupRecord {
            id: "ok".into(),
            name: "ok.txt".into(),
            primary_path: "/ok.txt".into(),
            mime_type: "text/plain".into(),
            owner: "owner@example.test".into(),
            status: "downloaded".into(),
            local_path: Some(temp_dir.path().join("ok.txt").display().to_string()),
            local_size: Some(2),
            source_modified_time: None,
            web_view_link: None,
            export_mime_type: None,
            recovery_method: None,
            reason: None,
            generated_at: "2026-01-01T00:00:00Z".into(),
        };
        let unresolved = SharedBackupRecord {
            id: "blocked".into(),
            name: "blocked".into(),
            primary_path: "/blocked".into(),
            mime_type: GOOGLE_EARTH_MIME.into(),
            owner: "owner@example.test".into(),
            status: "error".into(),
            local_path: None,
            local_size: None,
            source_modified_time: None,
            web_view_link: None,
            export_mime_type: None,
            recovery_method: None,
            reason: Some("manual export required".into()),
            generated_at: "2026-01-01T00:00:00Z".into(),
        };
        append_manifest_record(&manifest, &success).expect("success record");
        append_manifest_record(&manifest, &unresolved).expect("unresolved record");

        let backed_up = backed_up_ids_from_manifest(&manifest).expect("backed up ids");
        assert!(backed_up.contains("ok"));
        assert!(!backed_up.contains("blocked"));

        let unresolved_records =
            unresolved_records_from_manifest(&manifest).expect("unresolved records");
        assert_eq!(unresolved_records.len(), 1);
        assert_eq!(unresolved_records[0].id, "blocked");
    }
}
