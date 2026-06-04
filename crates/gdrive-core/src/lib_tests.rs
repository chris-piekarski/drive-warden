use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;

#[derive(Default, Clone)]
struct TestRepository {
    state: Arc<Mutex<Option<SyncState>>>,
    snapshot: Arc<Mutex<FullSnapshot>>,
    audit_log: Arc<Mutex<Vec<AuditLogEntry>>>,
    revoked_shares: Arc<Mutex<Vec<RevokedShareEntry>>>,
    trashed_files: Arc<Mutex<Vec<TrashedFileEntry>>>,
    moved_files: Arc<Mutex<Vec<MovedFileEntry>>>,
    created_folders: Arc<Mutex<Vec<CreatedFolderEntry>>>,
}

impl InventoryRepository for TestRepository {
    fn get_sync_state(&self) -> CoreResult<Option<SyncState>> {
        Ok(self.state.lock().expect("state").clone())
    }

    fn load_snapshot(&self) -> CoreResult<FullSnapshot> {
        Ok(self.snapshot.lock().expect("snapshot").clone())
    }

    fn load_inventory_items(&self) -> CoreResult<Vec<InventoryItem>> {
        Ok(build_inventory_items(&self.snapshot.lock().expect("snapshot")))
    }

    fn inspect_file(&self, id: &str) -> CoreResult<Option<InventoryItem>> {
        Ok(self.load_inventory_items()?.into_iter().find(|item| item.file.id == id))
    }

    fn append_audit_log(&self, entry: &AuditLogEntry) -> CoreResult<()> {
        self.audit_log.lock().expect("audit").push(entry.clone());
        Ok(())
    }

    fn load_audit_log(&self) -> CoreResult<Vec<AuditLogEntry>> {
        Ok(self.audit_log.lock().expect("audit").clone())
    }

    fn append_revoked_share(&self, entry: &RevokedShareEntry) -> CoreResult<()> {
        self.revoked_shares.lock().expect("revoked").push(entry.clone());
        Ok(())
    }

    fn load_revoked_shares(&self) -> CoreResult<Vec<RevokedShareEntry>> {
        Ok(self.revoked_shares.lock().expect("revoked").clone())
    }

    fn append_trashed_file(&self, entry: &TrashedFileEntry) -> CoreResult<()> {
        self.trashed_files.lock().expect("trashed").push(entry.clone());
        Ok(())
    }

    fn load_trashed_files(&self) -> CoreResult<Vec<TrashedFileEntry>> {
        Ok(self.trashed_files.lock().expect("trashed").clone())
    }

    fn append_moved_file(&self, entry: &MovedFileEntry) -> CoreResult<()> {
        self.moved_files.lock().expect("moved").push(entry.clone());
        Ok(())
    }

    fn load_moved_files(&self) -> CoreResult<Vec<MovedFileEntry>> {
        Ok(self.moved_files.lock().expect("moved").clone())
    }

    fn append_created_folder(&self, entry: &CreatedFolderEntry) -> CoreResult<()> {
        self.created_folders.lock().expect("created").push(entry.clone());
        Ok(())
    }

    fn load_created_folders(&self) -> CoreResult<Vec<CreatedFolderEntry>> {
        Ok(self.created_folders.lock().expect("created").clone())
    }

    fn begin_sync_run(
        &self,
        _account: &AccountProfile,
        _active_scopes: &[DriveScope],
        mode: SyncMode,
        source_page_token: Option<&str>,
    ) -> CoreResult<SyncRun> {
        Ok(SyncRun {
            run_id: "run-1".into(),
            mode,
            generation: 1,
            source_page_token: source_page_token.map(ToOwned::to_owned),
            started_at: Utc::now(),
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
    ) -> CoreResult<SyncSummary> {
        *self.snapshot.lock().expect("snapshot") = snapshot.clone();
        *self.state.lock().expect("state") = Some(SyncState {
            account: account.clone(),
            active_scopes: active_scopes.to_vec(),
            committed_start_page_token: Some(committed_page_token.to_string()),
            committed_generation: run.generation,
            last_sync_status: SyncStatus::Committed,
        });
        Ok(SyncSummary {
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

type MovedFileCall = (String, String, Vec<String>);

#[derive(Default, Clone)]
struct TestGateway {
    file_pages: Vec<FileListPage>,
    change_pages: Arc<Mutex<Vec<ChangeListPage>>>,
    session: Option<AuthSession>,
    deleted_permissions: Arc<Mutex<Vec<(String, String)>>>,
    trashed_files: Arc<Mutex<Vec<String>>>,
    moved_files: Arc<Mutex<Vec<MovedFileCall>>>,
    created_folders: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait]
impl DriveGateway for TestGateway {
    async fn login(&self, _scope: DriveScope) -> CoreResult<AuthSession> {
        self.session.clone().ok_or_else(|| CoreError::Message("no session".into()))
    }

    async fn logout(&self) -> CoreResult<bool> {
        Ok(true)
    }

    async fn auth_status(&self) -> CoreResult<AuthStatus> {
        Ok(AuthStatus { session: self.session.clone() })
    }

    async fn list_files(&self, page_token: Option<&str>) -> CoreResult<FileListPage> {
        let index = page_token.and_then(|raw| raw.parse::<usize>().ok()).unwrap_or(0);
        Ok(self.file_pages.get(index).cloned().unwrap_or_default())
    }

    async fn get_start_page_token(&self) -> CoreResult<String> {
        Ok("seed-token".into())
    }

    async fn list_changes(&self, _page_token: &str) -> CoreResult<ChangeListPage> {
        let page = self.change_pages.lock().expect("changes").remove(0);
        Ok(page)
    }

    async fn get_file(&self, id: &str) -> CoreResult<FileRecord> {
        Ok(FileRecord {
            id: id.to_string(),
            name: "Photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            image_media_metadata: Some(ImageMediaMetadata {
                width: Some(10),
                height: Some(20),
                ..ImageMediaMetadata::default()
            }),
            ..FileRecord::default()
        })
    }

    async fn inspect_exif(&self, id: &str) -> CoreResult<InspectExifDetails> {
        Ok(InspectExifDetails {
            file_id: id.to_string(),
            name: "Photo.jpg".into(),
            mime_type: "image/jpeg".into(),
            web_view_link: None,
            source: ExifSource::DownloadedBytes,
            metadata: ImageMediaMetadata {
                width: Some(10),
                height: Some(20),
                ..ImageMediaMetadata::default()
            },
        })
    }

    async fn ensure_scope(&self, _scope: DriveScope) -> CoreResult<()> {
        Ok(())
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord> {
        self.created_folders
            .lock()
            .expect("created folders")
            .push((parent_id.to_string(), name.to_string()));
        Ok(FileRecord {
            id: format!("created-folder-{name}"),
            name: name.to_string(),
            mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
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
            id: format!("copied-{file_id}"),
            name: name.unwrap_or(file_id).to_string(),
            mime_type: "text/plain".into(),
            parents: vec![parent_id.to_string()],
            owned_by_me: true,
            operator_can_share_manage: true,
            ..FileRecord::default()
        })
    }

    async fn delete_permission(&self, file_id: &str, permission_id: &str) -> CoreResult<()> {
        self.deleted_permissions
            .lock()
            .expect("deleted permissions")
            .push((file_id.to_string(), permission_id.to_string()));
        Ok(())
    }

    async fn trash_file(&self, file_id: &str) -> CoreResult<()> {
        self.trashed_files.lock().expect("trashed files").push(file_id.to_string());
        Ok(())
    }

    async fn find_file_in_folder(
        &self,
        _parent_id: &str,
        _name: &str,
    ) -> CoreResult<Option<RemoteFileMetadata>> {
        Ok(None)
    }

    async fn move_file(
        &self,
        file_id: &str,
        add_parent_id: &str,
        remove_parent_ids: &[String],
    ) -> CoreResult<RemoteFileMetadata> {
        self.moved_files.lock().expect("moved files").push((
            file_id.to_string(),
            add_parent_id.to_string(),
            remove_parent_ids.to_vec(),
        ));
        Ok(RemoteFileMetadata {
            id: file_id.to_string(),
            name: file_id.to_string(),
            mime_type: "text/plain".into(),
            size: None,
            modified_time: None,
            owned_by_me: true,
            shared: false,
            permissions: Vec::new(),
        })
    }
}

fn dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value).expect("datetime").with_timezone(&Utc)
}

fn sample_state() -> SyncState {
    SyncState {
        account: AccountProfile {
            account_id: "account-1".into(),
            email: "operator@example.com".into(),
            display_name: Some("Operator".into()),
        },
        active_scopes: vec![DriveScope::MetadataReadonly],
        committed_start_page_token: Some("token-1".into()),
        committed_generation: 1,
        last_sync_status: SyncStatus::Committed,
    }
}

#[test]
fn helper_string_and_filter_branches_are_covered() {
    assert_eq!(SyncStatus::Never.as_str(), "never");
    assert_eq!(SyncStatus::InProgress.as_str(), "in_progress");
    assert_eq!(SyncStatus::Committed.as_str(), "committed");
    assert_eq!(SyncStatus::Failed.as_str(), "failed");
    assert_eq!(ExifSource::DownloadedBytes.as_str(), "downloaded_bytes");
    assert_eq!(UnshareReasonCode::NotActionable.as_str(), "not_actionable");
    assert_eq!(UnshareReasonCode::NotOwnedOrManageable.as_str(), "not_owned_or_manageable");

    assert!(contains_ignore_case("Archive.zip", "archive"));
    assert!(simple_glob_match("/Docs/*.jpg", "/Docs/Photo.jpg"));
    assert!(simple_glob_match("/Docs/Photo.???", "/Docs/Photo.jpg"));
    assert!(!simple_glob_match("/Docs/*.png", "/Docs/Photo.jpg"));

    assert!(permission_matches_filter(
        &[PermissionRecord {
            permission_type: "domain".into(),
            domain: Some("example.com".into()),
            ..PermissionRecord::default()
        }],
        &SharedWithFilter::Domain("example.com".into())
    ));
    assert!(permission_matches_filter(
        &[PermissionRecord {
            permission_type: "user".into(),
            email_address: Some("user@example.com".into()),
            ..PermissionRecord::default()
        }],
        &SharedWithFilter::Email("user@example.com".into())
    ));
    assert_eq!(
        permission_target_label(&PermissionRecord {
            permission_type: "anyone".into(),
            allow_file_discovery: true,
            ..PermissionRecord::default()
        }),
        "anyone (discoverable)"
    );
    assert_eq!(
        permission_target_label(&PermissionRecord {
            permission_type: "domain".into(),
            ..PermissionRecord::default()
        }),
        "domain:unknown"
    );
    assert_eq!(
        permission_target_label(&PermissionRecord {
            permission_type: "user".into(),
            ..PermissionRecord::default()
        }),
        "user:unknown"
    );
    assert_eq!(
        permission_target_label(&PermissionRecord {
            permission_type: "group".into(),
            ..PermissionRecord::default()
        }),
        "group:unknown"
    );

    let io_error = std::io::Error::other("boom");
    assert_eq!(CoreError::from(io_error).to_string(), "boom");
    let json_error = serde_json::from_str::<serde_json::Value>("{").expect_err("json error");
    assert!(CoreError::from(json_error).to_string().contains("EOF"));
}

#[test]
fn repository_queries_and_inspection_cover_private_paths() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
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
                id: "name-a".into(),
                name: "Notes.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                size: Some(100),
                modified_time: Some(dt("2020-01-01T00:00:00Z")),
                ..FileRecord::default()
            },
            FileRecord {
                id: "name-b".into(),
                name: "Notes.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                size: Some(100),
                modified_time: Some(dt("2020-01-02T00:00:00Z")),
                ..FileRecord::default()
            },
            FileRecord {
                id: "public".into(),
                name: "Photo.jpg".into(),
                mime_type: "image/jpeg".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                size: Some(4096),
                viewed_by_me_time: Some(dt("2020-01-03T00:00:00Z")),
                permissions: vec![
                    PermissionRecord {
                        id: "perm-anyone".into(),
                        permission_type: "anyone".into(),
                        actionable: true,
                        ..PermissionRecord::default()
                    },
                    PermissionRecord {
                        id: "perm-no-email".into(),
                        permission_type: "user".into(),
                        actionable: false,
                        ..PermissionRecord::default()
                    },
                ],
                ..FileRecord::default()
            },
            FileRecord {
                id: "domain".into(),
                name: "Plan.docx".into(),
                mime_type:
                    "application/vnd.openxmlformats-officedocument.wordprocessingml.document".into(),
                parents: vec!["folder".into()],
                owned_by_me: false,
                shared: true,
                operator_can_share_manage: false,
                size: Some(5000),
                permissions: vec![
                    PermissionRecord {
                        id: "perm-domain".into(),
                        permission_type: "domain".into(),
                        domain: Some("example.com".into()),
                        actionable: true,
                        ..PermissionRecord::default()
                    },
                    PermissionRecord {
                        id: "perm-external".into(),
                        permission_type: "user".into(),
                        email_address: Some("outside@other.test".into()),
                        inherited: true,
                        actionable: false,
                        ..PermissionRecord::default()
                    },
                ],
                ..FileRecord::default()
            },
            FileRecord {
                id: "rootless".into(),
                name: "Loose.txt".into(),
                mime_type: "text/plain".into(),
                parents: Vec::new(),
                owned_by_me: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "orphan".into(),
                name: "Orphan.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["missing".into()],
                owned_by_me: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "cycle".into(),
                name: "Loop.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["cycle".into()],
                owned_by_me: true,
                ..FileRecord::default()
            },
        ],
    };

    let items = inventory_items(
        &repository,
        &InventoryQuery {
            owner_scope: OwnerScope::Mine,
            name_contains: Some("Notes".into()),
            mime_contains: Some("text".into()),
            in_folder: Some("folder".into()),
            path_glob: Some("/Docs/*".into()),
            larger_than: Some(50),
            older_than_days: Some(30),
            limit: Some(5),
            ..InventoryQuery::default()
        },
    )
    .expect("inventory items");
    assert_eq!(items.len(), 2);

    let duplicates = duplicate_groups(
        &repository,
        &InventoryQuery { duplicate_of: Some("name-a".into()), ..InventoryQuery::default() },
    )
    .expect("duplicates");
    assert_eq!(duplicates[0].match_type, DuplicateMatchType::NameSize);

    let inspect_missing = inspect_file_details(&repository, "missing").expect("inspect");
    assert!(inspect_missing.is_none());

    let inspect_public =
        inspect_file_details(&repository, "public").expect("inspect public").expect("details");
    assert_eq!(inspect_public.sharing_findings.len(), 1);

    let sharing = sharing_findings(
        &repository,
        &InventoryQuery {
            shared_only: true,
            shared_with: Some(SharedWithFilter::Domain("example.com".into())),
            ..InventoryQuery::default()
        },
    )
    .expect("sharing");
    assert_eq!(sharing.len(), 1);
    assert_eq!(sharing[0].kind, SharingKind::Domain);

    let actionable_only = sharing_findings(
        &repository,
        &InventoryQuery { actionable_only: true, shared_only: true, ..InventoryQuery::default() },
    )
    .expect("actionable");
    assert!(actionable_only.iter().all(|finding| finding.permission.actionable));

    let storage = storage_summary(
        &repository,
        &InventoryQuery { offset: 1, limit: Some(1), ..InventoryQuery::default() },
        30,
    )
    .expect("storage");
    assert_eq!(storage.large_files.len(), 1);
    assert_eq!(storage.large_files[0].item.file.id, "public");
    assert_eq!(storage.total_files, 8);

    let paths = build_path_entries(&repository.load_snapshot().expect("snapshot"));
    assert!(paths.iter().any(|entry| entry.path_state == PathState::Resolved));
    assert!(paths.iter().any(|entry| entry.path_state == PathState::Orphaned));
    assert!(paths.iter().any(|entry| entry.primary_path == "/Loose.txt"));

    let public_item = inspect_public.item.clone();
    assert!(!inventory_item_matches_query(
        &public_item,
        &InventoryQuery { mime_contains: Some("pdf".into()), ..InventoryQuery::default() }
    ));
    assert!(!inventory_item_matches_query(
        &public_item,
        &InventoryQuery { older_than_days: Some(9999), ..InventoryQuery::default() }
    ));
    assert!(!inventory_item_matches_query(
        &public_item,
        &InventoryQuery { in_folder: Some("other-folder".into()), ..InventoryQuery::default() }
    ));
    assert!(!inventory_item_matches_query(
        &public_item,
        &InventoryQuery { path_glob: Some("/Elsewhere/*".into()), ..InventoryQuery::default() }
    ));

    let email_filtered = sharing_findings(
        &repository,
        &InventoryQuery {
            shared_only: true,
            shared_with: Some(SharedWithFilter::Email("outside@other.test".into())),
            ..InventoryQuery::default()
        },
    )
    .expect("email filtered");
    assert_eq!(email_filtered.len(), 1);
    assert_eq!(email_filtered[0].kind, SharingKind::ExternalEmail);

    let unknown_permission_findings = build_sharing_findings(
        vec![InventoryItem {
            file: FileRecord {
                id: "group-share".into(),
                name: "Group.txt".into(),
                mime_type: "text/plain".into(),
                permissions: vec![PermissionRecord {
                    permission_type: "group".into(),
                    ..PermissionRecord::default()
                }],
                ..FileRecord::default()
            },
            path: PathEntry {
                file_id: "group-share".into(),
                primary_path: "/Group.txt".into(),
                all_paths: vec!["/Group.txt".into()],
                depth: 1,
                path_state: PathState::Resolved,
            },
        }],
        Some("operator@example.com"),
        &InventoryQuery { actionable_only: true, ..InventoryQuery::default() },
    );
    assert!(unknown_permission_findings.is_empty());
}

#[test]
fn unshare_and_change_helpers_cover_remaining_paths() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![FileRecord {
            id: "file-1".into(),
            name: "Shared.txt".into(),
            mime_type: "text/plain".into(),
            parents: vec!["root".into()],
            shared: true,
            operator_can_share_manage: false,
            permissions: vec![PermissionRecord {
                id: "perm-1".into(),
                permission_type: "user".into(),
                email_address: Some("outside@other.test".into()),
                actionable: false,
                ..PermissionRecord::default()
            }],
            ..FileRecord::default()
        }],
    };

    let gateway = TestGateway {
        session: Some(AuthSession {
            account: sample_state().account,
            active_scopes: vec![DriveScope::MetadataReadonly, DriveScope::Drive],
        }),
        ..TestGateway::default()
    };

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_unshare(&gateway, &repository, &InventoryQuery::default(), "unshare"))
        .expect("apply");
    assert_eq!(summary.applied, 0);
    assert_eq!(summary.skipped, 1);

    let direct_reason = classify_unshare_reason(&SharingFinding {
        item: build_inventory_items(&repository.load_snapshot().expect("snapshot"))
            .into_iter()
            .next()
            .expect("item"),
        permission: PermissionRecord { actionable: false, ..PermissionRecord::default() },
        kind: SharingKind::ExternalEmail,
        target_label: "outside@other.test".into(),
        actionable: false,
    });
    assert_eq!(direct_reason, UnshareReasonCode::NotOwnedOrManageable);

    let mut snapshot = FullSnapshot {
        files: vec![FileRecord {
            id: "keep".into(),
            name: "Keep.txt".into(),
            ..FileRecord::default()
        }],
    };
    let mut stats = SyncStats::default();
    apply_change_page(
        &mut snapshot,
        &ChangeListPage {
            next_page_token: None,
            new_start_page_token: Some("final-token".into()),
            removed_file_ids: vec!["keep".into()],
            updated_files: vec![
                FileRecord { id: "new".into(), name: "New.txt".into(), ..FileRecord::default() },
                FileRecord {
                    id: "trash".into(),
                    name: "Trash.txt".into(),
                    trashed: true,
                    ..FileRecord::default()
                },
            ],
        },
        &mut stats,
    );
    assert_eq!(stats.removed, 1);
    assert_eq!(stats.added, 1);

    let not_actionable_reason = classify_unshare_reason(&SharingFinding {
        item: build_inventory_items(&FullSnapshot {
            files: vec![FileRecord {
                id: "managed".into(),
                operator_can_share_manage: true,
                ..FileRecord::default()
            }],
        })
        .into_iter()
        .next()
        .expect("managed item"),
        permission: PermissionRecord { actionable: false, ..PermissionRecord::default() },
        kind: SharingKind::InternalEmail,
        target_label: "teammate@example.com".into(),
        actionable: false,
    });
    assert_eq!(not_actionable_reason, UnshareReasonCode::NotActionable);

    let files = BTreeMap::from([(
        "folder".to_string(),
        FileRecord {
            id: "folder".into(),
            name: "Docs".into(),
            parents: vec!["root".into()],
            ..FileRecord::default()
        },
    )]);
    let mut cache = BTreeMap::new();
    let mut orphan_cache = BTreeMap::new();
    let (missing_paths, missing_orphan) = resolve_paths_for_file(
        &files,
        &mut cache,
        &mut orphan_cache,
        "missing",
        &mut BTreeSet::new(),
    );
    assert!(missing_orphan);
    assert_eq!(missing_paths, vec!["[orphan]/missing".to_string()]);

    let (folder_paths, _) = resolve_paths_for_file(
        &files,
        &mut cache,
        &mut orphan_cache,
        "folder",
        &mut BTreeSet::new(),
    );
    assert_eq!(folder_paths, vec!["/Docs".to_string()]);
}

fn shared_user_permission(id: &str, role: &str, actionable: bool) -> PermissionRecord {
    PermissionRecord {
        id: id.into(),
        permission_type: "user".into(),
        role: role.into(),
        email_address: Some("user@partner.test".into()),
        actionable,
        ..PermissionRecord::default()
    }
}

#[test]
fn unshare_cascades_folder_inherited_grant_and_records_history() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "team-folder".into(),
                name: "Team".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                permissions: vec![shared_user_permission("perm-team", "writer", true)],
                ..FileRecord::default()
            },
            FileRecord {
                id: "doc-a".into(),
                name: "A.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["team-folder".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                permissions: vec![shared_user_permission("perm-team", "writer", false)],
                ..FileRecord::default()
            },
            FileRecord {
                id: "doc-b".into(),
                name: "B.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["team-folder".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                permissions: vec![shared_user_permission("perm-team", "writer", false)],
                ..FileRecord::default()
            },
        ],
    };

    let gateway = TestGateway {
        session: Some(AuthSession {
            account: sample_state().account,
            active_scopes: vec![DriveScope::MetadataReadonly, DriveScope::Drive],
        }),
        ..TestGateway::default()
    };
    let query = InventoryQuery {
        shared_with: Some(SharedWithFilter::Email("user@partner.test".into())),
        ..InventoryQuery::default()
    };

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_unshare(&gateway, &repository, &query, "unshare"))
        .expect("apply");

    // One API delete at the source folder; all three affected files recorded.
    assert_eq!(summary.applied, 3);
    let deletes = gateway.deleted_permissions.lock().expect("deletes").clone();
    assert_eq!(deletes, vec![("team-folder".to_string(), "perm-team".to_string())]);

    let history = repository.load_revoked_shares().expect("history");
    assert_eq!(history.len(), 3);
    let folder_entry =
        history.iter().find(|entry| entry.file_id == "team-folder").expect("folder entry");
    assert_eq!(folder_entry.source_folder_id, None);
    assert_eq!(folder_entry.role, "writer");
    assert_eq!(folder_entry.grantee, "user@partner.test");
    let child_entry = history.iter().find(|entry| entry.file_id == "doc-a").expect("child entry");
    assert_eq!(child_entry.source_folder_id.as_deref(), Some("team-folder"));
    assert!(child_entry.inherited);
    let audit = repository.load_audit_log().expect("audit");
    assert_eq!(audit.len(), 4);
    assert!(audit.iter().any(|entry| entry.action == "delete_permission_pending"));
    assert_eq!(audit.iter().filter(|entry| entry.action == "delete_permission").count(), 3);
}

#[test]
fn unshare_flags_grantee_owned_parent_as_unrevokable() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "grantee-folder".into(),
                name: "TheirFolder".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: false,
                shared: true,
                operator_can_share_manage: false,
                permissions: vec![shared_user_permission("perm-owner", "owner", false)],
                ..FileRecord::default()
            },
            FileRecord {
                id: "my-doc".into(),
                name: "Mine.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["grantee-folder".into()],
                owned_by_me: true,
                shared: true,
                operator_can_share_manage: true,
                permissions: vec![shared_user_permission("perm-owner", "writer", false)],
                ..FileRecord::default()
            },
        ],
    };

    let query = InventoryQuery {
        shared_with: Some(SharedWithFilter::Email("user@partner.test".into())),
        ..InventoryQuery::default()
    };
    let plan = unshare_plan(&repository, &query).expect("plan");
    let my_doc_row = plan.rows.iter().find(|row| row.item.file.id == "my-doc").expect("my-doc row");
    assert_eq!(my_doc_row.reason, UnshareReasonCode::GranteeOwnedParent);
    assert!(!my_doc_row.actionable);
    assert_eq!(plan.actionable_count, 0);
}

#[test]
fn trash_plan_and_apply_respect_recursive_and_actionable_guards() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "folder".into(),
                name: "Model".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "artifact".into(),
                name: "model.ckpt-1.data-00000-of-00001".into(),
                mime_type: "application/octet-stream".into(),
                parents: vec!["folder".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                size: Some(100),
                ..FileRecord::default()
            },
            FileRecord {
                id: "foreign".into(),
                name: "Vendor.bin".into(),
                mime_type: "application/octet-stream".into(),
                parents: vec!["root".into()],
                owned_by_me: false,
                operator_can_share_manage: false,
                size: Some(200),
                ..FileRecord::default()
            },
        ],
    };

    let no_recursive_plan = trash_plan(
        &repository,
        &InventoryQuery { path_glob: Some("/Model".into()), ..InventoryQuery::default() },
        &TrashOptions { recursive: false },
    )
    .expect("trash plan");
    assert_eq!(no_recursive_plan.actionable_count, 0);
    assert_eq!(no_recursive_plan.rows[0].reason, TrashReasonCode::FolderWithoutRecursive);
    assert_eq!(no_recursive_plan.rows[0].descendant_file_count, 1);

    let recursive_plan = trash_plan(
        &repository,
        &InventoryQuery { path_glob: Some("/Model*".into()), ..InventoryQuery::default() },
        &TrashOptions { recursive: true },
    )
    .expect("recursive trash plan");
    assert_eq!(recursive_plan.actionable_count, 1);
    assert_eq!(recursive_plan.rows.len(), 1);
    assert_eq!(recursive_plan.rows[0].item.file.id, "folder");
    assert_eq!(recursive_plan.total_bytes, 100);

    let foreign_plan = trash_plan(
        &repository,
        &InventoryQuery { name_contains: Some("Vendor".into()), ..InventoryQuery::default() },
        &TrashOptions::default(),
    )
    .expect("foreign trash plan");
    assert_eq!(foreign_plan.actionable_count, 0);
    assert_eq!(foreign_plan.rows[0].reason, TrashReasonCode::NotOwnedOrManageable);

    let gateway = TestGateway::default();
    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_trash(
            &gateway,
            &repository,
            &InventoryQuery { path_glob: Some("/Model*".into()), ..InventoryQuery::default() },
            &TrashOptions { recursive: true },
            "trash",
        ))
        .expect("apply trash");
    assert_eq!(summary.applied, 1);
    assert_eq!(gateway.trashed_files.lock().expect("trashed files").as_slice(), ["folder"]);
    let audit = repository.load_audit_log().expect("audit");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].action, "trash_file_pending");
    assert_eq!(audit[0].file_id, "folder");
    assert_eq!(audit[1].action, "trash_file");
    assert_eq!(audit[1].file_id, "folder");
    let history = repository.load_trashed_files().expect("trash history");
    assert_eq!(history.len(), 2);
    let root_entry = history.iter().find(|entry| entry.file_id == "folder").expect("root entry");
    assert!(root_entry.explicitly_requested);
    assert_eq!(root_entry.descendant_file_count, 1);
    assert!(root_entry.recoverable_until.expect("recovery") > root_entry.at);
    let child_entry =
        history.iter().find(|entry| entry.file_id == "artifact").expect("child entry");
    assert!(!child_entry.explicitly_requested);
    assert_eq!(child_entry.trashed_via_file_id.as_deref(), Some("folder"));
    assert_eq!(child_entry.trashed_via_path.as_deref(), Some("/Model"));
}

#[test]
fn move_plan_and_apply_record_pending_and_applied_history() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "docs".into(),
                name: "Docs".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "archive".into(),
                name: "Archive".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "nested".into(),
                name: "Nested".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["docs".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "report".into(),
                name: "Report.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["docs".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "rootless".into(),
                name: "Loose.txt".into(),
                mime_type: "text/plain".into(),
                parents: Vec::new(),
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
        ],
    };

    let actionable = move_plan(
        &repository,
        &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
        "archive",
    )
    .expect("move plan");
    assert_eq!(actionable.actionable_count, 1);
    assert_eq!(actionable.rows[0].reason, MoveReasonCode::Actionable);
    assert_eq!(actionable.rows[0].from_parent_ids, ["docs"]);
    assert_eq!(actionable.rows[0].to_parent_id, "archive");

    let current_parent = move_plan(
        &repository,
        &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
        "docs",
    )
    .expect("current parent plan");
    assert_eq!(current_parent.rows[0].reason, MoveReasonCode::DestinationIsCurrentParent);

    let rootless = move_plan(
        &repository,
        &InventoryQuery { file_id: Some("rootless".into()), ..InventoryQuery::default() },
        "archive",
    )
    .expect("rootless plan");
    assert_eq!(rootless.rows[0].reason, MoveReasonCode::Actionable);
    assert_eq!(rootless.rows[0].from_parent_ids, ["root"]);

    let into_descendant = move_plan(
        &repository,
        &InventoryQuery { file_id: Some("docs".into()), ..InventoryQuery::default() },
        "nested",
    )
    .expect("descendant plan");
    assert_eq!(into_descendant.rows[0].reason, MoveReasonCode::DestinationInsideSourceSubtree);

    let gateway = TestGateway::default();
    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_move(
            &gateway,
            &repository,
            &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
            "archive",
            "move",
        ))
        .expect("apply move");
    assert_eq!(summary.applied, 1);
    assert_eq!(
        gateway.moved_files.lock().expect("moved files").as_slice(),
        [("report".into(), "archive".into(), vec!["docs".into()])]
    );
    let audit = repository.load_audit_log().expect("audit");
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[0].action, "move_file_pending");
    assert_eq!(audit[1].action, "move_file");
    let history = repository.load_moved_files().expect("move history");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].status, "pending");
    assert_eq!(history[1].status, "applied");
    assert_eq!(history[1].from_parent_ids, ["docs"]);
    assert_eq!(history[1].to_parent_id, "archive");
    assert_eq!(history[1].from_path, "/Docs/Report.txt");
    assert_eq!(history[1].to_path, "/Archive");
}

#[test]
fn remote_db_manifest_decisions_and_privacy_validation_are_covered() {
    let bytes = b"sqlite snapshot";
    let manifest = build_remote_db_manifest(
        "inventory.db",
        bytes,
        Some("db-id-1".into()),
        Some(7),
        None,
        Some(12),
        Some(3),
        Some("tester@host".into()),
    );
    assert_eq!(manifest.sha256, sha256_hex(bytes));
    assert_eq!(manifest.byte_len, bytes.len() as u64);
    assert_eq!(manifest.db_instance_id.as_deref(), Some("db-id-1"));
    assert_eq!(manifest.db_generation, Some(7));
    assert!(verify_remote_db_manifest(&manifest, bytes).is_ok());
    assert!(verify_remote_db_manifest(&manifest, b"different").is_err());

    assert_eq!(decide_remote_db_sync(true, false), RemoteDbSyncDecision::PushLocal);
    assert_eq!(decide_remote_db_sync(false, true), RemoteDbSyncDecision::PullRemote);
    assert_eq!(decide_remote_db_sync(true, true), RemoteDbSyncDecision::NeedsExplicitDirection);
    assert_eq!(decide_remote_db_sync(false, false), RemoteDbSyncDecision::NothingToSync);

    let private_file = RemoteFileMetadata {
        id: "private".into(),
        name: "inventory.db".into(),
        mime_type: "application/vnd.sqlite3".into(),
        size: Some(1),
        modified_time: None,
        owned_by_me: true,
        shared: false,
        permissions: vec![PermissionRecord {
            id: "owner".into(),
            permission_type: "user".into(),
            role: "owner".into(),
            email_address: Some("me@example.test".into()),
            ..PermissionRecord::default()
        }],
    };
    assert!(validate_remote_db_privacy(&[private_file]).is_empty());

    let shared_file = RemoteFileMetadata {
        id: "shared".into(),
        name: "inventory.db".into(),
        mime_type: "application/vnd.sqlite3".into(),
        size: Some(1),
        modified_time: None,
        owned_by_me: true,
        shared: true,
        permissions: vec![PermissionRecord {
            id: "anyone".into(),
            permission_type: "anyone".into(),
            role: "reader".into(),
            ..PermissionRecord::default()
        }],
    };
    let issues = validate_remote_db_privacy(&[shared_file]);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].permission_type, "anyone");

    let not_owned_file = RemoteFileMetadata {
        id: "not-owned".into(),
        name: "inventory.db".into(),
        mime_type: "application/vnd.sqlite3".into(),
        size: Some(1),
        modified_time: None,
        owned_by_me: false,
        shared: false,
        permissions: vec![PermissionRecord {
            id: "owner".into(),
            permission_type: "user".into(),
            role: "owner".into(),
            email_address: Some("someone-else@example.test".into()),
            ..PermissionRecord::default()
        }],
    };
    let issues = validate_remote_db_privacy(&[not_owned_file]);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].permission_id, "ownedByMe");
}

#[test]
fn async_helpers_cover_snapshot_and_delta_paging() {
    let gateway = TestGateway {
        file_pages: vec![
            FileListPage {
                next_page_token: Some("1".into()),
                files: vec![FileRecord {
                    id: "folder".into(),
                    name: "Docs".into(),
                    mime_type: "application/vnd.google-apps.folder".into(),
                    ..FileRecord::default()
                }],
            },
            FileListPage {
                next_page_token: None,
                files: vec![FileRecord {
                    id: "file".into(),
                    name: "Photo.jpg".into(),
                    mime_type: "image/jpeg".into(),
                    ..FileRecord::default()
                }],
            },
        ],
        change_pages: Arc::new(Mutex::new(vec![
            ChangeListPage {
                next_page_token: Some("next".into()),
                new_start_page_token: None,
                removed_file_ids: Vec::new(),
                updated_files: vec![FileRecord {
                    id: "updated".into(),
                    name: "Updated.txt".into(),
                    ..FileRecord::default()
                }],
            },
            ChangeListPage {
                next_page_token: None,
                new_start_page_token: Some("done".into()),
                removed_file_ids: Vec::new(),
                updated_files: Vec::new(),
            },
        ])),
        session: Some(AuthSession {
            account: sample_state().account,
            active_scopes: vec![DriveScope::MetadataReadonly, DriveScope::DriveReadonly],
        }),
        ..TestGateway::default()
    };

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let snapshot = runtime.block_on(collect_full_snapshot(&gateway)).expect("snapshot");
    assert_eq!(snapshot.files.len(), 2);

    let (updated_snapshot, stats, token) = runtime
        .block_on(apply_delta_to_snapshot(&gateway, snapshot, "seed-token".into()))
        .expect("delta");
    assert_eq!(stats.added, 1);
    assert_eq!(token, "done");
    assert!(updated_snapshot.files.iter().any(|file| file.id == "updated"));

    let exif = runtime.block_on(inspect_exif(&gateway, "file")).expect("exif");
    assert_eq!(exif.source, ExifSource::DownloadedBytes);
}

#[test]
fn sync_failure_paths_mark_runs_failed() {
    #[derive(Default, Clone)]
    struct TrackingRepository {
        failed_messages: Arc<Mutex<Vec<String>>>,
    }

    impl InventoryRepository for TrackingRepository {
        fn get_sync_state(&self) -> CoreResult<Option<SyncState>> {
            Ok(Some(sample_state()))
        }

        fn load_snapshot(&self) -> CoreResult<FullSnapshot> {
            Ok(FullSnapshot::default())
        }

        fn load_inventory_items(&self) -> CoreResult<Vec<InventoryItem>> {
            Ok(Vec::new())
        }

        fn inspect_file(&self, _id: &str) -> CoreResult<Option<InventoryItem>> {
            Ok(None)
        }

        fn append_audit_log(&self, _entry: &AuditLogEntry) -> CoreResult<()> {
            Ok(())
        }

        fn load_audit_log(&self) -> CoreResult<Vec<AuditLogEntry>> {
            Ok(Vec::new())
        }

        fn begin_sync_run(
            &self,
            _account: &AccountProfile,
            _active_scopes: &[DriveScope],
            mode: SyncMode,
            source_page_token: Option<&str>,
        ) -> CoreResult<SyncRun> {
            Ok(SyncRun {
                run_id: "run".into(),
                mode,
                generation: 1,
                source_page_token: source_page_token.map(ToOwned::to_owned),
                started_at: Utc::now(),
            })
        }

        fn replace_snapshot(
            &self,
            _run: &SyncRun,
            _account: &AccountProfile,
            _active_scopes: &[DriveScope],
            _snapshot: &FullSnapshot,
            _committed_page_token: &str,
            _stats: SyncStats,
        ) -> CoreResult<SyncSummary> {
            Err(CoreError::Message("unexpected replace".into()))
        }

        fn mark_sync_failed(&self, _run: &SyncRun, message: &str) -> CoreResult<()> {
            self.failed_messages.lock().expect("failed messages").push(message.to_string());
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FailingGateway {
        session: AuthSession,
        fail_delta: bool,
    }

    #[async_trait]
    impl DriveGateway for FailingGateway {
        async fn login(&self, _scope: DriveScope) -> CoreResult<AuthSession> {
            Ok(self.session.clone())
        }
        async fn logout(&self) -> CoreResult<bool> {
            Ok(true)
        }
        async fn auth_status(&self) -> CoreResult<AuthStatus> {
            Ok(AuthStatus { session: Some(self.session.clone()) })
        }
        async fn list_files(&self, _page_token: Option<&str>) -> CoreResult<FileListPage> {
            Ok(FileListPage::default())
        }
        async fn get_start_page_token(&self) -> CoreResult<String> {
            if self.fail_delta {
                Ok("seed-token".into())
            } else {
                Err(CoreError::Message("full failure".into()))
            }
        }
        async fn list_changes(&self, _page_token: &str) -> CoreResult<ChangeListPage> {
            Err(CoreError::Message("delta failure".into()))
        }
        async fn get_file(&self, _id: &str) -> CoreResult<FileRecord> {
            unreachable!()
        }
        async fn inspect_exif(&self, _id: &str) -> CoreResult<InspectExifDetails> {
            unreachable!()
        }
        async fn ensure_scope(&self, _scope: DriveScope) -> CoreResult<()> {
            Ok(())
        }
        async fn create_folder(&self, parent_id: &str, name: &str) -> CoreResult<FileRecord> {
            Ok(FileRecord {
                id: format!("folder-{name}"),
                name: name.to_string(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec![parent_id.to_string()],
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
                ..FileRecord::default()
            })
        }
        async fn delete_permission(&self, _file_id: &str, _permission_id: &str) -> CoreResult<()> {
            Ok(())
        }
    }

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let repository = TrackingRepository::default();
    let session = AuthSession {
        account: sample_state().account,
        active_scopes: vec![DriveScope::MetadataReadonly],
    };

    let full_error = runtime
        .block_on(run_full_sync(
            &FailingGateway { session: session.clone(), fail_delta: false },
            &repository,
            &session,
        ))
        .expect_err("full failure");
    assert_eq!(full_error.to_string(), "full failure");

    let delta_session = AuthSession {
        account: sample_state().account,
        active_scopes: vec![DriveScope::MetadataReadonly],
    };
    let delta_error = runtime
        .block_on(run_delta_sync(
            &FailingGateway { session, fail_delta: true },
            &repository,
            &delta_session,
            "token-1".into(),
        ))
        .expect_err("delta failure");
    assert_eq!(delta_error.to_string(), "delta failure");
    assert_eq!(repository.failed_messages.lock().expect("failed messages").len(), 2);
}

#[test]
fn sync_inventory_falls_back_to_full_when_delta_token_expires() {
    #[derive(Clone)]
    struct ExpiredDeltaGateway {
        session: AuthSession,
    }

    #[async_trait]
    impl DriveGateway for ExpiredDeltaGateway {
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
            if page_token.is_some() {
                return Ok(FileListPage::default());
            }
            Ok(FileListPage {
                next_page_token: None,
                files: vec![FileRecord {
                    id: "fresh".into(),
                    name: "Fresh.txt".into(),
                    mime_type: "text/plain".into(),
                    ..FileRecord::default()
                }],
            })
        }
        async fn get_start_page_token(&self) -> CoreResult<String> {
            Ok("fresh-token".into())
        }
        async fn list_changes(&self, page_token: &str) -> CoreResult<ChangeListPage> {
            if page_token == "fresh-token" {
                return Ok(ChangeListPage {
                    new_start_page_token: Some("fresh-token".into()),
                    ..ChangeListPage::default()
                });
            }
            Err(CoreError::Message(
                "410 Gone from Google Drive changes feed while listing Google Drive changes".into(),
            ))
        }
        async fn get_file(&self, _id: &str) -> CoreResult<FileRecord> {
            unreachable!()
        }
        async fn inspect_exif(&self, _id: &str) -> CoreResult<InspectExifDetails> {
            unreachable!()
        }
        async fn ensure_scope(&self, _scope: DriveScope) -> CoreResult<()> {
            Ok(())
        }
        async fn create_folder(&self, _parent_id: &str, _name: &str) -> CoreResult<FileRecord> {
            unreachable!()
        }
        async fn copy_file(
            &self,
            _file_id: &str,
            _parent_id: &str,
            _name: Option<&str>,
        ) -> CoreResult<FileRecord> {
            unreachable!()
        }
        async fn delete_permission(&self, _file_id: &str, _permission_id: &str) -> CoreResult<()> {
            unreachable!()
        }
    }

    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    let gateway = ExpiredDeltaGateway {
        session: AuthSession {
            account: sample_state().account,
            active_scopes: vec![DriveScope::MetadataReadonly],
        },
    };

    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(sync_inventory(&gateway, &repository, false))
        .expect("fallback full sync");
    assert_eq!(summary.mode, SyncMode::Full);
    assert_eq!(summary.committed_page_token, "fresh-token");
    assert_eq!(summary.file_count, 1);
}

#[test]
fn move_plan_supports_root_destination_and_provisioning_preview() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "docs".into(),
                name: "Docs".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "report".into(),
                name: "Report.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["docs".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
        ],
    };

    let to_root = move_orchestration_plan(
        &repository,
        &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
        MoveDestinationTarget::Root,
        &MoveOptions::default(),
    )
    .expect("root plan");
    assert_eq!(to_root.move_plan.actionable_count, 1);
    assert_eq!(to_root.move_plan.rows[0].to_parent_id, MY_DRIVE_ROOT_ID);

    let provision = move_orchestration_plan(
        &repository,
        &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
        MoveDestinationTarget::Path("/Archive/New".into()),
        &MoveOptions { provision_missing: true },
    )
    .expect("provision plan");
    let provisioning = provision.provisioning.expect("provisioning");
    assert_eq!(provisioning.create_count, 2);
    assert_eq!(provisioning.destination_path, "/Archive/New");
}

#[test]
fn apply_move_orchestration_provisions_destination_and_records_history() {
    let repository = TestRepository::default();
    *repository.state.lock().expect("state") = Some(sample_state());
    *repository.snapshot.lock().expect("snapshot") = FullSnapshot {
        files: vec![
            FileRecord {
                id: "archive".into(),
                name: "Archive".into(),
                mime_type: GOOGLE_DRIVE_FOLDER_MIME.into(),
                parents: vec!["root".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
            FileRecord {
                id: "report".into(),
                name: "Report.txt".into(),
                mime_type: "text/plain".into(),
                parents: vec!["archive".into()],
                owned_by_me: true,
                operator_can_share_manage: true,
                ..FileRecord::default()
            },
        ],
    };
    let gateway = TestGateway::default();
    let summary = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(apply_move_orchestration(
            &gateway,
            &repository,
            &InventoryQuery { file_id: Some("report".into()), ..InventoryQuery::default() },
            MoveDestinationTarget::Path("/Archive/New".into()),
            &MoveOptions { provision_missing: true },
            "move",
        ))
        .expect("apply orchestrated move");
    assert_eq!(summary.provisioning_created, 1);
    assert_eq!(summary.move_summary.applied, 1);
    assert_eq!(
        gateway.created_folders.lock().expect("created").as_slice(),
        [("archive".into(), "New".into())]
    );
    let folder_history = repository.load_created_folders().expect("folder history");
    assert_eq!(folder_history.len(), 2);
    assert_eq!(folder_history[0].status, "pending");
    assert_eq!(folder_history[1].status, "applied");
}
