use std::fs;
use std::path::Path;

use chrono::Utc;
use gdrive_core::{
    CoreResult, DuplicateGroup, InventoryItem, ReportWriter, SharingFinding, StorageSummary,
    SyncState,
};

#[derive(Debug, Default)]
pub struct MarkdownReportWriter;

impl ReportWriter for MarkdownReportWriter {
    fn write_markdown(&self, path: &str, contents: &str) -> CoreResult<()> {
        if let Some(parent_dir) = Path::new(path).parent() {
            fs::create_dir_all(parent_dir)?;
        }
        fs::write(path, contents)?;
        Ok(())
    }
}

pub fn render_summary_report(
    sync_state: Option<&SyncState>,
    items: &[InventoryItem],
    duplicates: &[DuplicateGroup],
    sharing: &[SharingFinding],
    storage: &StorageSummary,
) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");
    let duplicate_files = duplicates.iter().map(|group| group.items.len()).sum::<usize>();
    let public_links = sharing.iter().filter(|finding| finding.kind.as_str() == "anyone").count();
    let stale_files = storage.stale_files.len();

    format!(
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: summary\n---\n\n## Executive summary\n\n- Total files in snapshot: **{}**\n- Duplicate groups: **{}** covering **{}** files\n- Sharing findings: **{}**\n- Public links: **{}**\n- Stale files: **{}**\n- Total tracked bytes: **{}**\n\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Files | {} |\n| Duplicate groups | {} |\n| Sharing findings | {} |\n| Public links | {} |\n| Total bytes | {} |\n\n## Recommended actions\n\n1. Run `gdrive-optimize find duplicates` to inspect duplicate candidates.\n2. Run `gdrive-optimize find shared --shared-with anyone` to review public links.\n3. Run `gdrive-optimize find large --min 1048576` to inspect large files.\n\n## Appendix\n\nThis summary is generated from the local SQLite snapshot only.\n",
        items.len(),
        duplicates.len(),
        duplicate_files,
        sharing.len(),
        public_links,
        stale_files,
        storage.total_bytes,
        items.len(),
        duplicates.len(),
        sharing.len(),
        public_links,
        storage.total_bytes
    )
}

pub fn render_duplicates_report(
    sync_state: Option<&SyncState>,
    duplicates: &[DuplicateGroup],
) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");
    let mut details = String::new();

    for group in duplicates {
        details.push_str(&format!(
            "### Group `{}` ({})\n\n| File ID | Name | Path |\n|---------|------|------|\n",
            group.group_key,
            group.match_type.as_str()
        ));
        for item in &group.items {
            details.push_str(&format!(
                "| {} | {} | {} |\n",
                item.file.id, item.file.name, item.path.primary_path
            ));
        }
        details.push('\n');
    }

    format!(
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: duplicates\n---\n\n## Executive summary\n\n- Duplicate groups: **{}**\n\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Duplicate groups | {} |\n\n## Detailed findings\n\n{}## Recommended actions\n\n1. Review grouped files above.\n2. Keep the preferred copy and remove or archive the rest later.\n\n## Appendix\n\nGroups are built by MD5 first, then by name+size when checksums are absent.\n",
        duplicates.len(),
        duplicates.len(),
        details
    )
}

pub fn render_sharing_report(
    sync_state: Option<&SyncState>,
    findings: &[SharingFinding],
) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");
    let mut details =
        String::from("| File ID | Name | Path | Kind | Target | Actionable |\n|---------|------|------|------|--------|------------|\n");
    for finding in findings {
        details.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            finding.item.file.id,
            finding.item.file.name,
            finding.item.path.primary_path,
            finding.kind.as_str(),
            finding.target_label,
            if finding.actionable { "yes" } else { "no" }
        ));
    }

    format!(
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: sharing\n---\n\n## Executive summary\n\n- Sharing findings: **{}**\n\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Sharing findings | {} |\n\n## Detailed findings\n\n{}\n## Recommended actions\n\n1. Review public and external sharing rows first.\n2. Preview actionable rows with `gdrive-optimize unshare --shared-with anyone` before applying changes.\n\n## Appendix\n\nRows include both actionable and informational sharing states.\n",
        findings.len(),
        findings.len(),
        details
    )
}

pub fn render_storage_report(sync_state: Option<&SyncState>, storage: &StorageSummary) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");

    let mut large_details =
        String::from("| File ID | Name | Path | Bytes |\n|---------|------|------|-------|\n");
    for finding in &storage.large_files {
        large_details.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            finding.item.file.id,
            finding.item.file.name,
            finding.item.path.primary_path,
            finding.size_bytes
        ));
    }

    let mut stale_details = String::from(
        "| File ID | Name | Path | Stale days |\n|---------|------|------|------------|\n",
    );
    for finding in &storage.stale_files {
        stale_details.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            finding.item.file.id,
            finding.item.file.name,
            finding.item.path.primary_path,
            finding.stale_days.unwrap_or(0)
        ));
    }

    format!(
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: storage\n---\n\n## Executive summary\n\n- Total files: **{}**\n- Total tracked bytes: **{}**\n- Large file rows: **{}**\n- Stale file rows: **{}** (threshold: {} days)\n\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Files | {} |\n| Total bytes | {} |\n| Large files | {} |\n| Stale files | {} |\n\n## Detailed findings\n\n### Largest files\n\n{}\n### Stale files\n\n{}\n## Recommended actions\n\n1. Review the largest files for archive or cleanup.\n2. Review stale files that have not been touched in {}+ days.\n\n## Appendix\n\nStorage analysis is based on locally synced metadata only.\n",
        storage.total_files,
        storage.total_bytes,
        storage.large_files.len(),
        storage.stale_files.len(),
        storage.stale_threshold_days,
        storage.total_files,
        storage.total_bytes,
        storage.large_files.len(),
        storage.stale_files.len(),
        large_details,
        stale_details,
        storage.stale_threshold_days
    )
}

#[cfg(test)]
mod lib_tests;
