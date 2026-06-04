use std::fs;
use std::path::Path;

use chrono::Utc;
use gdrive_core::{
    AccountAbout, CoreResult, DuplicateGroup, InventoryItem, ReportWriter, SharingFinding,
    StorageSummary, SyncState,
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

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn render_account_about_executive_lines(about: &AccountAbout) -> String {
    let quota = &about.quota;
    let usage_line = if let Some(limit) = quota.limit {
        let free = quota.free_bytes().unwrap_or(0);
        format!(
            "- Account storage used: **{}** / **{}** (**{}** free)\n",
            format_bytes(quota.usage),
            format_bytes(limit),
            format_bytes(free),
        )
    } else {
        format!(
            "- Account storage used: **{}** (unlimited or pooled plan)\n",
            format_bytes(quota.usage),
        )
    };
    let trash_line = if let Some(pct) = about.trash_pct_of_limit() {
        format!(
            "- Trash reclaimable: **{}** ({pct:.1}% of quota)\n",
            format_bytes(about.trash_reclaimable_bytes),
        )
    } else {
        format!("- Trash reclaimable: **{}**\n", format_bytes(about.trash_reclaimable_bytes),)
    };
    let max_upload_line = about
        .max_upload_size
        .map(|size| format!("- Max upload size: **{}**\n", format_bytes(size)))
        .unwrap_or_default();
    format!(
        "{usage_line}- Active Drive files: **{}**\n- Non-Drive Google usage: **{}**\n{trash_line}- Drive file usage (incl. trash): **{}**\n{max_upload_line}",
        format_bytes(about.active_drive_bytes),
        format_bytes(about.non_drive_bytes),
        format_bytes(quota.usage_in_drive),
    )
}

fn render_account_about_metrics_rows(about: &AccountAbout) -> String {
    let quota = &about.quota;
    let limit = quota.limit.map(format_bytes).unwrap_or_else(|| "unlimited".into());
    let free = quota.free_bytes().map(format_bytes).unwrap_or_else(|| "n/a".into());
    let max_upload = about.max_upload_size.map(format_bytes).unwrap_or_else(|| "unknown".into());
    let trash_pct =
        about.trash_pct_of_limit().map(|pct| format!("{pct:.1}%")).unwrap_or_else(|| "n/a".into());
    format!(
        "| Account usage | {} |\n| Account limit | {} |\n| Account free | {} |\n| Active Drive files | {} |\n| Non-Drive Google usage | {} |\n| Trash reclaimable | {} |\n| Trash % of quota | {} |\n| Drive usage (incl. trash) | {} |\n| Max upload size | {} |\n",
        format_bytes(quota.usage),
        limit,
        free,
        format_bytes(about.active_drive_bytes),
        format_bytes(about.non_drive_bytes),
        format_bytes(about.trash_reclaimable_bytes),
        trash_pct,
        format_bytes(quota.usage_in_drive),
        max_upload,
    )
}

fn render_account_about_appendix(about: Option<&AccountAbout>) -> &'static str {
    match about {
        Some(_) => "Snapshot byte totals may differ slightly from live Google account quota. Account figures are fetched live from Google Drive `about.get`.",
        None => "This report is generated from the local SQLite snapshot. Live account settings were unavailable (not logged in or backend does not support about lookup).",
    }
}

pub fn render_summary_report(
    sync_state: Option<&SyncState>,
    items: &[InventoryItem],
    duplicates: &[DuplicateGroup],
    sharing: &[SharingFinding],
    storage: &StorageSummary,
    account_about: Option<&AccountAbout>,
) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");
    let duplicate_files = duplicates.iter().map(|group| group.items.len()).sum::<usize>();
    let public_links = sharing.iter().filter(|finding| finding.kind.as_str() == "anyone").count();
    let stale_files = storage.stale_files.len();
    let quota_executive =
        account_about.map(render_account_about_executive_lines).unwrap_or_default();
    let quota_metrics = account_about.map(render_account_about_metrics_rows).unwrap_or_default();

    format!(
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: summary\n---\n\n## Executive summary\n\n- Total files in snapshot: **{}**\n- Duplicate groups: **{}** covering **{}** files\n- Sharing findings: **{}**\n- Public links: **{}**\n- Stale files: **{}**\n- Total tracked bytes: **{}**\n{quota_executive}\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Files | {} |\n| Duplicate groups | {} |\n| Sharing findings | {} |\n| Public links | {} |\n| Total bytes | {} |\n{quota_metrics}\n## Recommended actions\n\n1. Run `drive-warden find duplicates` to inspect duplicate candidates.\n2. Run `drive-warden find shared --shared-with anyone` to review public links.\n3. Run `drive-warden find large --min 1048576` to inspect large files.\n\n## Appendix\n\n{}\n",
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
        storage.total_bytes,
        render_account_about_appendix(account_about),
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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: sharing\n---\n\n## Executive summary\n\n- Sharing findings: **{}**\n\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Sharing findings | {} |\n\n## Detailed findings\n\n{}\n## Recommended actions\n\n1. Review public and external sharing rows first.\n2. Preview actionable rows with `drive-warden unshare --shared-with anyone` before applying changes.\n\n## Appendix\n\nRows include both actionable and informational sharing states.\n",
        findings.len(),
        findings.len(),
        details
    )
}

pub fn render_storage_report(
    sync_state: Option<&SyncState>,
    storage: &StorageSummary,
    account_about: Option<&AccountAbout>,
) -> String {
    let generated_at = Utc::now().to_rfc3339();
    let account_email = sync_state.map(|state| state.account.email.as_str()).unwrap_or("unknown");
    let quota_executive =
        account_about.map(render_account_about_executive_lines).unwrap_or_default();
    let quota_metrics = account_about.map(render_account_about_metrics_rows).unwrap_or_default();

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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: storage\n---\n\n## Executive summary\n\n- Total files: **{}**\n- Total tracked bytes: **{}**\n- Large file rows: **{}**\n- Stale file rows: **{}** (threshold: {} days)\n{quota_executive}\n## Metrics dashboard\n\n| Metric | Value |\n|--------|-------|\n| Files | {} |\n| Total bytes | {} |\n| Large files | {} |\n| Stale files | {} |\n{quota_metrics}\n## Detailed findings\n\n### Largest files\n\n{}\n### Stale files\n\n{}\n## Recommended actions\n\n1. Review the largest files for archive or cleanup.\n2. Review stale files that have not been touched in {}+ days.\n\n## Appendix\n\n{}\n",
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
        storage.stale_threshold_days,
        render_account_about_appendix(account_about),
    )
}

#[cfg(test)]
mod lib_tests;
