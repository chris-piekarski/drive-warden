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
            "- Facility quota consumed: **{}** / **{}** (**{}** remaining)\n",
            format_bytes(quota.usage),
            format_bytes(limit),
            format_bytes(free),
        )
    } else {
        format!(
            "- Facility quota consumed: **{}** (unlimited or pooled plan)\n",
            format_bytes(quota.usage),
        )
    };
    let trash_line = if let Some(pct) = about.trash_pct_of_limit() {
        format!(
            "- Segregation hold (trash) reclaimable: **{}** ({pct:.1}% of quota)\n",
            format_bytes(about.trash_reclaimable_bytes),
        )
    } else {
        format!(
            "- Segregation hold (trash) reclaimable: **{}**\n",
            format_bytes(about.trash_reclaimable_bytes),
        )
    };
    let max_upload_line = about
        .max_upload_size
        .map(|size| format!("- Intake size limit (max upload): **{}**\n", format_bytes(size)))
        .unwrap_or_default();
    format!(
        "{usage_line}- Active inmates (Drive files): **{}**\n- Off-block Google services usage: **{}**\n{trash_line}- Cell block usage (incl. trash): **{}**\n{max_upload_line}",
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
        "| Facility quota consumed | {} |\n| Facility quota limit | {} |\n| Facility quota remaining | {} |\n| Active inmates | {} |\n| Off-block Google usage | {} |\n| Segregation hold reclaimable | {} |\n| Segregation hold % of quota | {} |\n| Cell block usage (incl. trash) | {} |\n| Intake size limit | {} |\n",
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
        Some(_) => "Ledger byte totals may differ slightly from live facility quota. Quota figures are fetched live from Google Drive `about.get` under strict warden oversight.",
        None => "This briefing is generated from the local intake ledger only. Live facility settings were unavailable (warden off duty or backend does not support about lookup).",
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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: summary\nwarden: drive-warden\n---\n\n## Warden briefing\n\n- Inmates on the intake ledger: **{}**\n- Identity collision groups: **{}** covering **{}** inmates\n- Clearance violations flagged: **{}**\n- Public-access breaches: **{}**\n- Idle inmates (stale): **{}**\n- Ledger byte total: **{}**\n{quota_executive}\n## Block census\n\n| Metric | Value |\n|--------|-------|\n| Inmates | {} |\n| Identity collision groups | {} |\n| Clearance violations | {} |\n| Public-access breaches | {} |\n| Ledger bytes | {} |\n{quota_metrics}\n## Security orders\n\n1. Run `drive-warden find duplicates` to inspect identity collision groups.\n2. Run `drive-warden find shared --shared-with anyone` to review public-access breaches.\n3. Run `drive-warden find large --min 1048576` to inspect the heaviest inmates.\n\n## Warden's ledger notes\n\n{}\n",
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
            "### Collision group `{}` ({})\n\n| Inmate ID | Name | Cell path |\n|---------|------|-----------|\n",
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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: duplicates\nwarden: drive-warden\n---\n\n## Warden briefing\n\n- Identity collision groups: **{}**\n\n## Block census\n\n| Metric | Value |\n|--------|-------|\n| Identity collision groups | {} |\n\n## Cell inspection\n\n{}## Security orders\n\n1. Review collision groups above and decide which inmate record to keep.\n2. Retain the preferred copy; archive or segregate the rest under warden supervision.\n\n## Warden's ledger notes\n\nGroups are built by MD5 first, then by name+size when checksums are absent.\n",
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
    let mut details = String::from(
        "| Inmate ID | Name | Cell path | Clearance kind | Target | Warden actionable |\n|---------|------|-----------|----------------|--------|-------------------|\n",
    );
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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: sharing\nwarden: drive-warden\n---\n\n## Warden briefing\n\n- Clearance violations flagged: **{}**\n\n## Block census\n\n| Metric | Value |\n|--------|-------|\n| Clearance violations | {} |\n\n## Cell inspection\n\n{}\n## Security orders\n\n1. Review public and external clearance rows first.\n2. Preview warden-actionable rows with `drive-warden unshare --shared-with anyone` before revoking access.\n\n## Warden's ledger notes\n\nRows include both warden-actionable and informational clearance states.\n",
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

    let mut large_details = String::from(
        "| Inmate ID | Name | Cell path | Bytes |\n|---------|------|-----------|-------|\n",
    );
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
        "| Inmate ID | Name | Cell path | Idle days |\n|---------|------|-----------|-----------|\n",
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
        "---\ngenerated_at: {generated_at}\naccount: {account_email}\nreport: storage\nwarden: drive-warden\n---\n\n## Warden briefing\n\n- Inmates on the intake ledger: **{}**\n- Ledger byte total: **{}**\n- Heavy inmate rows: **{}**\n- Idle inmate rows: **{}** (threshold: {} days)\n{quota_executive}\n## Block census\n\n| Metric | Value |\n|--------|-------|\n| Inmates | {} |\n| Ledger bytes | {} |\n| Heavy inmates | {} |\n| Idle inmates | {} |\n{quota_metrics}\n## Cell inspection\n\n### Heaviest inmates\n\n{}\n### Idle inmates\n\n{}\n## Security orders\n\n1. Review the heaviest inmates for archive or segregation.\n2. Review idle inmates untouched for {}+ days.\n\n## Warden's ledger notes\n\n{}\n",
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
