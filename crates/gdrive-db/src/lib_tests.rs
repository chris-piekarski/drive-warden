use super::*;

#[test]
fn stats_helpers_cover_repository_accessors_and_parsers() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let db_path = temp_dir.path().join("inventory.db");
    let repository = SqliteInventoryRepository::new(&db_path).expect("repository");

    assert_eq!(repository.db_path(), db_path.as_path());
    assert!(repository.lookup_path_entry("missing").expect("lookup").is_none());

    let stats = repository.stats().expect("stats");
    assert_eq!(stats.db_path, db_path.display().to_string());
    assert_eq!(stats.file_count, 0);
    let identity = repository.db_identity().expect("identity");
    assert_eq!(identity.db_instance_id.len(), 32);
    assert_eq!(identity.schema_version, 1);
    let remote_state = repository.remote_sync_state().expect("remote state");
    assert_eq!(remote_state.generation, 0);

    let recorded = repository
        .record_remote_sync(
            RemoteSyncDirection::Push,
            Some(1),
            "remote-db-file",
            "abc123",
            Utc::now(),
            42,
            Some("tester@host"),
        )
        .expect("record push");
    assert_eq!(recorded.generation, 1);
    assert_eq!(recorded.last_remote_file_id.as_deref(), Some("remote-db-file"));
    assert_eq!(recorded.last_manifest_sha256.as_deref(), Some("abc123"));

    let trashed_at = Utc::now();
    repository
        .append_trashed_file(&TrashedFileEntry {
            at: trashed_at,
            recoverable_until: Some(trashed_at + chrono::Duration::days(30)),
            command: "trash".into(),
            file_id: "file-1".into(),
            file_name: "artifact.pb".into(),
            file_path: "[orphan]/Coors/Model/artifact.pb".into(),
            mime_type: "application/octet-stream".into(),
            size: Some(123),
            md5_checksum: Some("md5".into()),
            modified_time: Some(trashed_at),
            trashed_via_file_id: Some("folder-1".into()),
            trashed_via_path: Some("[orphan]/Coors/Model".into()),
            explicitly_requested: false,
            descendant_file_count: 0,
            descendant_folder_count: 0,
            trash_via: "tool".into(),
            note: Some("test".into()),
        })
        .expect("append trash history");
    let trash_history = repository.load_trashed_files().expect("load trash history");
    assert_eq!(trash_history.len(), 1);
    assert_eq!(trash_history[0].file_id, "file-1");
    assert_eq!(trash_history[0].trashed_via_file_id.as_deref(), Some("folder-1"));

    let vacuum = repository.vacuum().expect("vacuum");
    assert_eq!(vacuum.db_path, db_path.display().to_string());

    assert_eq!(parse_sync_status("failed"), SyncStatus::Failed);
    assert_eq!(parse_sync_status("unexpected"), SyncStatus::Never);
    assert_eq!(parse_path_state("resolved"), PathState::Resolved);
    assert!(parse_optional_datetime(None).expect("none").is_none());
}
