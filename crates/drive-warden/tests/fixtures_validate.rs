use std::path::Path;

#[test]
fn shared_fixture_directories_exist() {
    assert!(Path::new("../../tests/fixtures").exists());
    assert!(Path::new("../../tests/snapshots/cli").exists());
    assert!(Path::new("../../tests/config/mock.toml").exists());

    for dataset in [
        "drive_small",
        "drive_duplicates",
        "drive_sharing",
        "drive_paths",
        "drive_failures",
        "drive_reports",
    ] {
        let dataset_root = format!("../../tests/fixtures/{dataset}");
        let api_dir = format!("{dataset_root}/api");
        let expected_dir = format!("{dataset_root}/expected");
        let expected_db_dir = format!("{expected_dir}/db");
        let expected_cli_dir = format!("{expected_dir}/cli");
        let expected_reports_dir = format!("{expected_dir}/reports");

        assert!(Path::new(&dataset_root).exists(), "missing dataset root: {dataset_root}");
        assert!(Path::new(&api_dir).exists(), "missing api dir: {api_dir}");
        assert!(Path::new(&expected_db_dir).exists(), "missing expected db dir: {expected_db_dir}");
        assert!(
            Path::new(&expected_cli_dir).exists(),
            "missing expected cli dir: {expected_cli_dir}"
        );
        assert!(
            Path::new(&expected_reports_dir).exists(),
            "missing expected reports dir: {expected_reports_dir}"
        );
    }

    assert!(Path::new("../../tests/fixtures/drive_small/api/mock-drive.json").exists());
}
