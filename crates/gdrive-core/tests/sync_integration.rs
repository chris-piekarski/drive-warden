use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gdrive_core::{
    apply_unshare, apply_unshare_with_options, auth_status, duplicate_groups, inspect_exif, login,
    sharing_findings, storage_summary, sync_inventory, unshare_plan, unshare_plan_with_options,
    AccountProfile, AuditLogEntry, AuthSession, AuthStatus, ChangeListPage, CoreResult,
    DriveGateway, DriveScope, ExifSource, FileListPage, FileRecord, FullSnapshot,
    ImageMediaMetadata, InspectExifDetails, InventoryItem, InventoryQuery, InventoryRepository,
    PermissionRecord, RetainCopyOptions, SyncMode, SyncRun, SyncState, SyncStats, SyncStatus,
    UnshareReasonCode,
};

#[derive(Clone)]
struct FakeGateway {
    session: AuthSession,
}

#[async_trait]
impl DriveGateway for FakeGateway {
    async fn login(&self, _scope: DriveScope) -> CoreResult<AuthSession> {
        Ok(self.session.clone())
    }

    async fn logout(&self) -> CoreResult<bool> {
        Ok(true)
    }

    async fn auth_status(&self) -> CoreResult<AuthStatus> {
        Ok(AuthStatus { session: Some(self.session.clone()) })
    }

    async fn list_files(&self, page_token: Option<&str>) -> CoreResult<FileListPage> {
        match page_token {
            None => Ok(FileListPage {
                next_page_token: None,
                files: vec![FileRecord {
                    id: "folder".into(),
                    name: "Docs".into(),
                    mime_type: "application/vnd.google-apps.folder".into(),
                    parents: vec!["root".into()],
                    trashed: false,
                    owned_by_me: true,
                    ..FileRecord::default()
                }],
            }),
            Some(_) => Ok(FileListPage::default()),
        }
    }

    async fn get_start_page_token(&self) -> CoreResult<String> {
        Ok("token-1".into())
    }

    async fn list_changes(&self, page_token: &str) -> CoreResult<ChangeListPage> {
        match page_token {
            "token-1" => Ok(ChangeListPage {
                next_page_token: None,
                new_start_page_token: Some("token-2".into()),
                removed_file_ids: Vec::new(),
                updated_files: vec![FileRecord {
                    id: "report".into(),
                    name: "Report.txt".into(),
                    mime_type: "text/plain".into(),
                    parents: vec!["folder".into()],
                    trashed: false,
                    owned_by_me: true,
                    ..FileRecord::default()
                }],
            }),
            "token-2" => Ok(ChangeListPage {
                next_page_token: None,
                new_start_page_token: Some("token-3".into()),
                removed_file_ids: Vec::new(),
                updated_files: Vec::new(),
            }),
            other => panic!("unexpected token: {other}"),
        }
    }

    async fn get_file(&self, _id: &str) -> CoreResult<FileRecord> {
        unreachable!()
    }

    async fn inspect_exif(&self, id: &str) -> CoreResult<InspectExifDetails> {
        Ok(InspectExifDetails {
            file_id: id.to_string(),
            name: "Photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            web_view_link: Some("https://example.test/photo".into()),
            source: ExifSource::DriveImageMediaMetadata,
            metadata: ImageMediaMetadata {
                width: Some(4000),
                height: Some(3000),
                camera_make: Some("ExampleCam".into()),
                camera_model: Some("Model One".into()),
                date_taken: Some(
                    chrono::DateTime::parse_from_rfc3339("2024-04-01T12:00:00Z")
                        .expect("dt")
                        .with_timezone(&chrono::Utc),
                ),
                exposure_time: Some("1/125".into()),
                aperture: Some("f/2.8".into()),
                focal_length: Some("50mm".into()),
                iso_speed: Some(200),
            },
        })
    }

    async fn ensure_scope(&self, _scope: DriveScope) -> CoreResult<()> {
        Ok(())
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord> {
        Ok(FileRecord {
            id: format!("folder-{name}"),
            name: name.to_string(),
            mime_type: gdrive_core::GOOGLE_DRIVE_FOLDER_MIME.into(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            operator_can_share_manage: true,
            ..FileRecord::default()
        })
    }

    async fn copy_file(
        &self,
        file_id: &str,
        parent_id: &str,
        name: Option<&str>,
    ) -> CoreResult<FileRecord> {
        Ok(FileRecord {
            id: format!("copy-{file_id}"),
            name: name.unwrap_or(file_id).to_string(),
            mime_type: "text/plain".into(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            operator_can_share_manage: true,
            ..FileRecord::default()
        })
    }

    async fn delete_permission(&self, _file_id: &str, _permission_id: &str) -> CoreResult<()> {
        Ok(())
    }
}

#[derive(Default, Clone)]
struct FakeRepository {
    state: Arc<Mutex<Option<SyncState>>>,
    snapshot: Arc<Mutex<FullSnapshot>>,
    audit_log: Arc<Mutex<Vec<AuditLogEntry>>>,
}

impl InventoryRepository for FakeRepository {
    fn get_sync_state(&self) -> CoreResult<Option<SyncState>> {
        Ok(self.state.lock().expect("state lock").clone())
    }

    fn load_snapshot(&self) -> CoreResult<FullSnapshot> {
        Ok(self.snapshot.lock().expect("snapshot lock").clone())
    }

    fn load_inventory_items(&self) -> CoreResult<Vec<InventoryItem>> {
        Ok(gdrive_core::build_inventory_items(&self.snapshot.lock().expect("snapshot lock")))
    }

    fn inspect_file(&self, id: &str) -> CoreResult<Option<InventoryItem>> {
        Ok(self.load_inventory_items()?.into_iter().find(|item| item.file.id == id))
    }

    fn append_audit_log(&self, entry: &AuditLogEntry) -> CoreResult<()> {
        self.audit_log.lock().expect("audit lock").push(entry.clone());
        Ok(())
    }

    fn load_audit_log(&self) -> CoreResult<Vec<AuditLogEntry>> {
        Ok(self.audit_log.lock().expect("audit lock").clone())
    }

    fn begin_sync_run(
        &self,
        _account: &AccountProfile,
        _active_scopes: &[DriveScope],
        mode: SyncMode,
        source_page_token: Option<&str>,
    ) -> CoreResult<SyncRun> {
        let generation = self
            .state
            .lock()
            .expect("state lock")
            .as_ref()
            .map(|state| state.committed_generation + 1)
            .unwrap_or(1);
        Ok(SyncRun {
            run_id: format!("run-{generation}"),
            mode,
            generation,
            source_page_token: source_page_token.map(ToOwned::to_owned),
            started_at: chrono::Utc::now(),
        })
    }

    fn replace_snapshot(
        &self,
        run: &SyncRun,
        account: &AccountProfile,
        active_scopes: &[DriveScope],
        snapshot: &FullSnapshot,
        committed_page_token: &str,
        stats: SyncStats,
    ) -> CoreResult<gdrive_core::SyncSummary> {
        *self.snapshot.lock().expect("snapshot lock") = snapshot.clone();
        *self.state.lock().expect("state lock") = Some(SyncState {
            account: account.clone(),
            active_scopes: active_scopes.to_vec(),
            committed_start_page_token: Some(committed_page_token.to_string()),
            committed_generation: run.generation,
            last_sync_status: SyncStatus::Committed,
        });

        Ok(gdrive_core::SyncSummary {
            mode: run.mode,
            added: stats.added,
            updated: stats.updated,
            removed: stats.removed,
            committed_page_token: committed_page_token.to_string(),
            generation: run.generation,
            file_count: snapshot.files.len(),
            path_count: snapshot.files.len(),
        })
    }

    fn mark_sync_failed(&self, _run: &SyncRun, _message: &str) -> CoreResult<()> {
        Ok(())
    }
}

#[test]
fn sync_inventory_runs_full_then_delta() {
    let gateway = FakeGateway {
        session: AuthSession {
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "mock@example.com".into(),
                display_name: Some("Mock".into()),
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    };
    let repository = FakeRepository::default();

    let first = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(sync_inventory(&gateway, &repository, false))
        .expect("full sync");
    assert_eq!(first.mode, SyncMode::Full);
    assert_eq!(first.file_count, 2);
    assert_eq!(first.committed_page_token, "token-2");

    let second = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(sync_inventory(&gateway, &repository, false))
        .expect("delta sync");
    assert_eq!(second.mode, SyncMode::Delta);
    assert_eq!(second.committed_page_token, "token-3");
}

#[test]
fn sync_inventory_ignores_state_from_a_different_account() {
    let gateway = FakeGateway {
        session: AuthSession {
            account: AccountProfile {
                account_id: "account-2".into(),
                email: "second@example.com".into(),
                display_name: Some("Second".into()),
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    };
    let repository = FakeRepository::default();
    *repository.state.lock().expect("state lock") = Some(SyncState {
        account: AccountProfile {
            account_id: "account-1".into(),
            email: "first@example.com".into(),
            display_name: Some("First".into()),
        },
        active_scopes: vec![DriveScope::MetadataReadonly],
        committed_start_page_token: Some("wrong-account-token".into()),
        committed_generation: 7,
        last_sync_status: SyncStatus::Committed,
    });

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(sync_inventory(&gateway, &repository, false))
        .expect("full sync for new account");

    assert_eq!(summary.mode, SyncMode::Full);
    assert_eq!(summary.committed_page_token, "token-2");
    assert_eq!(
        repository.state.lock().expect("state lock").as_ref().expect("state").account.account_id,
        "account-2"
    );
}

#[test]
fn auth_helpers_delegate_to_gateway() {
    let gateway = FakeGateway {
        session: AuthSession {
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "mock@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    };

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let session = runtime.block_on(login(&gateway, DriveScope::MetadataReadonly)).expect("login");
    assert_eq!(session.account.email, "mock@example.com");

    let status = runtime.block_on(auth_status(&gateway)).expect("status");
    assert!(status.is_logged_in());

    let exif = runtime.block_on(inspect_exif(&gateway, "photo-1")).expect("exif");
    assert_eq!(exif.source, ExifSource::DriveImageMediaMetadata);
    assert_eq!(exif.metadata.camera_model.as_deref(), Some("Model One"));
}

#[test]
fn analysis_engines_detect_duplicates_sharing_and_storage() {
    let repository = FakeRepository::default();
    *repository.snapshot.lock().expect("snapshot lock") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "folder".into(),
                name: "Docs".into(),
                mime_type: "application/vnd.google-apps.folder".into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "dup-a".into(),
                name: "Archive.zip".into(),
                mime_type: "application/zip".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                size: Some(100),
                md5_checksum: Some("dup".into()),
                modified_time: Some(
                    chrono::DateTime::parse_from_rfc3339("2021-01-01T00:00:00Z")
                        .expect("dt")
                        .with_timezone(&chrono::Utc),
                ),
                ..FileRecord::default()
            },
            FileRecord {
                id: "dup-b".into(),
                name: "Archive Copy.zip".into(),
                mime_type: "application/zip".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                size: Some(100),
                md5_checksum: Some("dup".into()),
                modified_time: Some(
                    chrono::DateTime::parse_from_rfc3339("2021-01-02T00:00:00Z")
                        .expect("dt")
                        .with_timezone(&chrono::Utc),
                ),
                ..FileRecord::default()
            },
            FileRecord {
                id: "public".into(),
                name: "Public.pdf".into(),
                mime_type: "application/pdf".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                size: Some(4096),
                permissions: vec![PermissionRecord {
                    permission_type: "anyone".into(),
                    allow_file_discovery: false,
                    actionable: true,
                    ..PermissionRecord::default()
                }],
                ..FileRecord::default()
            },
        ],
    };
    *repository.state.lock().expect("state lock") = Some(SyncState {
        account: AccountProfile {
            account_id: "account-1".into(),
            email: "mock@example.com".into(),
            display_name: None,
        },
        active_scopes: vec![DriveScope::MetadataReadonly],
        committed_start_page_token: Some("token-1".into()),
        committed_generation: 1,
        last_sync_status: SyncStatus::Committed,
    });

    let query = InventoryQuery::default();
    let duplicates = duplicate_groups(&repository, &query).expect("duplicates");
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].items.len(), 2);

    let sharing = sharing_findings(&repository, &query).expect("sharing");
    assert_eq!(sharing.len(), 1);
    assert_eq!(sharing[0].target_label, "anyone with link");

    let storage = storage_summary(&repository, &query, 730).expect("storage");
    assert_eq!(storage.total_files, 4);
    assert!(!storage.large_files.is_empty());
    assert!(!storage.stale_files.is_empty());

    let largest_only = storage_summary(
        &repository,
        &InventoryQuery { limit: Some(1), ..InventoryQuery::default() },
        730,
    )
    .expect("largest only");
    assert_eq!(largest_only.large_files[0].item.file.id, "public");

    let first_sharing = sharing_findings(
        &repository,
        &InventoryQuery { limit: Some(1), ..InventoryQuery::default() },
    )
    .expect("first sharing");
    assert_eq!(first_sharing.len(), 1);
    assert_eq!(first_sharing[0].item.file.id, "public");
}

#[test]
fn unshare_preview_and_apply_use_actionable_rows_only() {
    let repository = FakeRepository::default();
    *repository.snapshot.lock().expect("snapshot lock") = FullSnapshot {
        files: vec![FileRecord {
            id: "public".into(),
            name: "Public.pdf".into(),
            mime_type: "application/pdf".into(),
            parents: vec!["root".into()],
            owned_by_me: true,
            shared: true,
            operator_can_share_manage: true,
            permissions: vec![
                PermissionRecord {
                    id: "perm-anyone".into(),
                    permission_type: "anyone".into(),
                    actionable: true,
                    ..PermissionRecord::default()
                },
                PermissionRecord {
                    id: "perm-inherited".into(),
                    permission_type: "user".into(),
                    email_address: Some("outside@example.test".into()),
                    inherited: true,
                    actionable: false,
                    ..PermissionRecord::default()
                },
            ],
            ..FileRecord::default()
        }],
    };
    *repository.state.lock().expect("state lock") = Some(SyncState {
        account: AccountProfile {
            account_id: "account-1".into(),
            email: "mock@example.com".into(),
            display_name: None,
        },
        active_scopes: vec![DriveScope::MetadataReadonly],
        committed_start_page_token: Some("token-1".into()),
        committed_generation: 1,
        last_sync_status: SyncStatus::Committed,
    });
    let gateway = FakeGateway {
        session: AuthSession {
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "mock@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly, DriveScope::Drive],
        },
    };

    let query = InventoryQuery::default();
    let plan = unshare_plan(&repository, &query).expect("plan");
    assert_eq!(plan.actionable_count, 1);
    assert_eq!(plan.skipped_count, 1);
    assert!(plan.rows.iter().any(|row| row.reason == UnshareReasonCode::Actionable));
    assert!(plan.rows.iter().any(|row| row.reason == UnshareReasonCode::InheritedPermission));

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_unshare(&gateway, &repository, &query, "unshare"))
        .expect("apply");
    assert_eq!(summary.applied, 1);
    let audit = repository.load_audit_log().expect("audit");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].action, "delete_permission_pending");
    assert_eq!(audit[1].action, "delete_permission");
}

#[test]
fn retained_copy_plan_and_apply_backup_folder_trees_before_unshare() {
    let repository = FakeRepository::default();
    *repository.snapshot.lock().expect("snapshot lock") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "shared-folder".into(),
                name: "SharedFolder".into(),
                mime_type: gdrive_core::GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: false,
                shared: true,
                operator_can_share_manage: true,
                permissions: vec![PermissionRecord {
                    id: "perm-anyone".into(),
                    permission_type: "anyone".into(),
                    actionable: true,
                    ..PermissionRecord::default()
                }],
                ..FileRecord::default()
            },
            FileRecord {
                id: "child-file".into(),
                name: "Child.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["shared-folder".into()],
                owned_by_me: false,
                shared: false,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
        ],
    };
    *repository.state.lock().expect("state lock") = Some(SyncState {
        account: AccountProfile {
            account_id: "account-1".into(),
            email: "mock@example.com".into(),
            display_name: None,
        },
        active_scopes: vec![DriveScope::MetadataReadonly],
        committed_start_page_token: Some("token-1".into()),
        committed_generation: 1,
        last_sync_status: SyncStatus::Committed,
    });
    let gateway = FakeGateway {
        session: AuthSession {
            account: AccountProfile {
                account_id: "account-1".into(),
                email: "mock@example.com".into(),
                display_name: None,
            },
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    };
    let query = InventoryQuery {
        shared_with: Some(gdrive_core::SharedWithFilter::Anyone),
        ..Default::default()
    };
    let retain_copy = RetainCopyOptions { enabled: true, backup_root_id: None };

    let plan = unshare_plan_with_options(&repository, &query, Some(&retain_copy)).expect("plan");
    let backup_plan = plan.retain_copy.expect("retain copy plan");
    assert_eq!(backup_plan.root_count, 1);
    assert_eq!(backup_plan.total_folder_copies, 1);
    assert_eq!(backup_plan.total_file_copies, 1);

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_unshare_with_options(
            &gateway,
            &repository,
            &query,
            Some(&retain_copy),
            "unshare",
        ))
        .expect("apply");
    let retain_summary = summary.retain_copy.expect("retain summary");
    assert_eq!(retain_summary.root_count, 1);
    assert_eq!(retain_summary.created_folders, 2);
    assert_eq!(retain_summary.copied_files, 1);

    let audit = repository.load_audit_log().expect("audit log");
    let actions = audit.iter().map(|entry| entry.action.as_str()).collect::<Vec<_>>();
    assert_eq!(
        actions,
        vec![
            "create_backup_folder",
            "create_backup_folder",
            "copy_backup_file",
            "delete_permission_pending",
            "delete_permission",
        ]
    );
    assert_eq!(audit[2].source_file_id.as_deref(), Some("child-file"));
    assert_eq!(audit[3].permission_id, "perm-anyone");
    assert_eq!(audit[4].permission_id, "perm-anyone");
}
