use super::*;
use gdrive_core::{AccountAbout, StorageQuota};

#[test]
fn markdown_writer_handles_paths_without_parent_directories() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let previous_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(temp_dir.path()).expect("set cwd");

    MarkdownReportWriter.write_markdown("report.md", "# report\n").expect("write markdown");

    let contents = std::fs::read_to_string(temp_dir.path().join("report.md")).expect("report");
    assert!(contents.contains("# report"));

    std::env::set_current_dir(previous_dir).expect("restore cwd");
}

#[test]
fn format_bytes_uses_binary_units() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(2048), "2.00 KiB");
    assert_eq!(format_bytes(5_368_709_120), "5.00 GiB");
}

fn sample_account_about() -> AccountAbout {
    AccountAbout::from_quota(
        StorageQuota {
            limit: Some(15_000_000_000),
            usage: 6_000_000_000,
            usage_in_drive: 5_500_000_000,
            usage_in_drive_trash: 500_000_000,
        },
        Some(5_368_709_120),
        Some(false),
    )
}

#[test]
fn summary_report_includes_live_account_about_when_present() {
    let storage = StorageSummary {
        total_files: 10,
        total_bytes: 1_000,
        large_files: Vec::new(),
        stale_files: Vec::new(),
        stale_threshold_days: 90,
    };
    let about = sample_account_about();

    let report = render_summary_report(None, &[], &[], &[], &storage, Some(&about));

    assert!(report.contains("Warden briefing"));
    assert!(report.contains("Facility quota consumed:"));
    assert!(report.contains("Active inmates (Drive files):"));
    assert!(report.contains("Off-block Google services usage:"));
    assert!(report.contains("Segregation hold (trash) reclaimable:"));
    assert!(report.contains("Intake size limit (max upload):"));
    assert!(report.contains("about.get"));
}

#[test]
fn storage_report_notes_unlimited_quota() {
    let storage = StorageSummary {
        total_files: 1,
        total_bytes: 100,
        large_files: Vec::new(),
        stale_files: Vec::new(),
        stale_threshold_days: 90,
    };
    let about = AccountAbout::from_quota(
        StorageQuota {
            limit: None,
            usage: 6_000_000_000,
            usage_in_drive: 5_500_000_000,
            usage_in_drive_trash: 0,
        },
        None,
        None,
    );

    let report = render_storage_report(None, &storage, Some(&about));

    assert!(report.contains("unlimited or pooled plan"));
    assert!(report.contains("| Facility quota limit | unlimited |"));
}
