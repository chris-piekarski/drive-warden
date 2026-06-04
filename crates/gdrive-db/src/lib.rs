use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use gdrive_core::{
    build_inventory_items, build_path_entries, AccountProfile, AuditLogEntry, CoreError,
    CoreResult, DriveScope, FileRecord, FullSnapshot, InventoryItem, InventoryRepository,
    MovedFileEntry, PathEntry, PathState, PermissionRecord, RevokedShareEntry, SyncMode, SyncRun,
    SyncState, SyncStats, SyncStatus, SyncSummary, TrashedFileEntry,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

mod embedded {
    use refinery::embed_migrations;

    embed_migrations!("../../migrations");
}

#[derive(Debug, Clone)]
pub struct SqliteInventoryRepository {
    db_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseStats {
    pub db_path: String,
    pub db_bytes: u64,
    pub file_count: usize,
    pub parent_count: usize,
    pub path_count: usize,
    pub sync_run_count: usize,
    pub audit_log_count: usize,
    pub committed_generation: Option<i64>,
    pub committed_page_token: Option<String>,
    pub last_sync_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VacuumResult {
    pub db_path: String,
    pub before_bytes: u64,
    pub after_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseSnapshotInfo {
    pub page_count: u64,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseIdentity {
    pub db_instance_id: String,
    pub created_at: DateTime<Utc>,
    pub last_opened_at: Option<DateTime<Utc>>,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteSyncState {
    pub generation: i64,
    pub last_pushed_at: Option<DateTime<Utc>>,
    pub last_pulled_at: Option<DateTime<Utc>>,
    pub last_remote_file_id: Option<String>,
    pub last_manifest_sha256: Option<String>,
    pub last_manifest_uploaded_at: Option<DateTime<Utc>>,
    pub last_remote_byte_len: Option<u64>,
    pub last_source_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSyncDirection {
    Push,
    Pull,
}

impl SqliteInventoryRepository {
    pub fn new<P: AsRef<Path>>(db_path: P) -> CoreResult<Self> {
        let repository = Self { db_path: db_path.as_ref().to_path_buf() };
        repository.ensure_parent_dir()?;
        let mut connection = repository.open_connection()?;
        embedded::migrations::runner()
            .run(&mut connection)
            .map_err(|error| CoreError::Message(error.to_string()))?;
        repository.touch_db_identity_opened_at()?;
        Ok(repository)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn lookup_path_entry(&self, file_id: &str) -> CoreResult<Option<PathEntry>> {
        let connection = self.open_connection()?;
        let row = connection
            .query_row(
                "SELECT primary_path, all_paths_json, depth, path_state FROM path_cache WHERE file_id = ?1",
                params![file_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? as usize,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| CoreError::Message(error.to_string()))?;

        row.map(|(primary_path, all_paths_json, depth, path_state)| {
            Ok(PathEntry {
                file_id: file_id.to_string(),
                primary_path,
                all_paths: serde_json::from_str(&all_paths_json)?,
                depth,
                path_state: parse_path_state(&path_state),
            })
        })
        .transpose()
    }

    fn ensure_parent_dir(&self) -> CoreResult<()> {
        if let Some(parent_dir) = self.db_path.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        Ok(())
    }

    fn open_connection(&self) -> CoreResult<Connection> {
        let connection = Connection::open(&self.db_path)
            .map_err(|error| CoreError::Message(error.to_string()))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| CoreError::Message(error.to_string()))?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| CoreError::Message(error.to_string()))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(connection)
    }

    pub fn stats(&self) -> CoreResult<DatabaseStats> {
        let connection = self.open_connection()?;
        let file_count = query_count(&connection, "SELECT COUNT(*) FROM files")?;
        let parent_count = query_count(&connection, "SELECT COUNT(*) FROM parents")?;
        let path_count = query_count(&connection, "SELECT COUNT(*) FROM path_cache")?;
        let sync_run_count = query_count(&connection, "SELECT COUNT(*) FROM sync_runs")?;
        let audit_log_count = query_count(&connection, "SELECT COUNT(*) FROM audit_log")?;
        let sync_state = connection
            .query_row(
                "SELECT committed_generation, committed_start_page_token, last_sync_status FROM sync_state LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let db_bytes = std::fs::metadata(&self.db_path).map(|metadata| metadata.len()).unwrap_or(0);

        Ok(DatabaseStats {
            db_path: self.db_path.display().to_string(),
            db_bytes,
            file_count,
            parent_count,
            path_count,
            sync_run_count,
            audit_log_count,
            committed_generation: sync_state.as_ref().map(|(generation, _, _)| *generation),
            committed_page_token: sync_state.as_ref().and_then(|(_, token, _)| token.clone()),
            last_sync_status: sync_state.map(|(_, _, status)| status),
        })
    }

    pub fn vacuum(&self) -> CoreResult<VacuumResult> {
        let before_bytes =
            std::fs::metadata(&self.db_path).map(|metadata| metadata.len()).unwrap_or(0);
        let connection = self.open_connection()?;
        connection
            .execute_batch("VACUUM")
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let after_bytes =
            std::fs::metadata(&self.db_path).map(|metadata| metadata.len()).unwrap_or(0);
        Ok(VacuumResult { db_path: self.db_path.display().to_string(), before_bytes, after_bytes })
    }

    pub fn snapshot_to<P: AsRef<Path>>(&self, destination: P) -> CoreResult<DatabaseSnapshotInfo> {
        let destination = destination.as_ref();
        if let Some(parent_dir) = destination.parent() {
            std::fs::create_dir_all(parent_dir)?;
        }
        if destination.exists() {
            std::fs::remove_file(destination)?;
        }
        let escaped = destination.to_string_lossy().replace('\'', "''");
        let connection = self.open_connection()?;
        connection
            .execute_batch(&format!("VACUUM INTO '{escaped}'"))
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Self::snapshot_info_for_path(destination)
    }

    pub fn snapshot_info(&self) -> CoreResult<DatabaseSnapshotInfo> {
        Self::snapshot_info_for_path(&self.db_path)
    }

    pub fn db_identity(&self) -> CoreResult<DatabaseIdentity> {
        let connection = self.open_connection()?;
        connection
            .query_row(
                "SELECT db_instance_id, created_at, last_opened_at, schema_version FROM db_identity WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(|error| CoreError::Message(error.to_string()))
            .and_then(|(db_instance_id, created_at, last_opened_at, schema_version)| {
                Ok(DatabaseIdentity {
                    db_instance_id,
                    created_at: parse_datetime(&created_at)?,
                    last_opened_at: parse_optional_datetime(last_opened_at)?,
                    schema_version: schema_version as u32,
                })
            })
    }

    pub fn remote_sync_state(&self) -> CoreResult<RemoteSyncState> {
        let connection = self.open_connection()?;
        connection
            .query_row(
                "SELECT generation, last_pushed_at, last_pulled_at, last_remote_file_id, last_manifest_sha256, last_manifest_uploaded_at, last_remote_byte_len, last_source_label FROM remote_sync_state WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .map_err(|error| CoreError::Message(error.to_string()))
            .and_then(
                |(
                    generation,
                    last_pushed_at,
                    last_pulled_at,
                    last_remote_file_id,
                    last_manifest_sha256,
                    last_manifest_uploaded_at,
                    last_remote_byte_len,
                    last_source_label,
                )| {
                    Ok(RemoteSyncState {
                        generation,
                        last_pushed_at: parse_optional_datetime(last_pushed_at)?,
                        last_pulled_at: parse_optional_datetime(last_pulled_at)?,
                        last_remote_file_id,
                        last_manifest_sha256,
                        last_manifest_uploaded_at: parse_optional_datetime(last_manifest_uploaded_at)?,
                        last_remote_byte_len: last_remote_byte_len.map(|value| value as u64),
                        last_source_label,
                    })
                },
            )
    }

    pub fn record_remote_sync(
        &self,
        direction: RemoteSyncDirection,
        synced_generation: Option<i64>,
        remote_file_id: &str,
        manifest_sha256: &str,
        manifest_uploaded_at: DateTime<Utc>,
        remote_byte_len: u64,
        source_label: Option<&str>,
    ) -> CoreResult<RemoteSyncState> {
        let connection = self.open_connection()?;
        let now = Utc::now().to_rfc3339();
        let manifest_uploaded_at = manifest_uploaded_at.to_rfc3339();
        let remote_byte_len = remote_byte_len as i64;
        match direction {
            RemoteSyncDirection::Push => {
                connection.execute(
                    "UPDATE remote_sync_state SET generation = COALESCE(?1, generation + 1), last_pushed_at = ?2, last_remote_file_id = ?3, last_manifest_sha256 = ?4, last_manifest_uploaded_at = ?5, last_remote_byte_len = ?6, last_source_label = ?7 WHERE id = 1",
                    params![synced_generation, now, remote_file_id, manifest_sha256, manifest_uploaded_at, remote_byte_len, source_label],
                )
            }
            RemoteSyncDirection::Pull => {
                connection.execute(
                    "UPDATE remote_sync_state SET generation = COALESCE(?1, generation), last_pulled_at = ?2, last_remote_file_id = ?3, last_manifest_sha256 = ?4, last_manifest_uploaded_at = ?5, last_remote_byte_len = ?6, last_source_label = ?7 WHERE id = 1",
                    params![synced_generation, now, remote_file_id, manifest_sha256, manifest_uploaded_at, remote_byte_len, source_label],
                )
            }
        }
        .map_err(|error| CoreError::Message(error.to_string()))?;
        self.remote_sync_state()
    }

    fn snapshot_info_for_path(path: &Path) -> CoreResult<DatabaseSnapshotInfo> {
        let connection =
            Connection::open(path).map_err(|error| CoreError::Message(error.to_string()))?;
        let page_count = query_pragma_u64(&connection, "page_count")?;
        let schema_version = query_pragma_u64(&connection, "schema_version")? as u32;
        Ok(DatabaseSnapshotInfo { page_count, schema_version })
    }

    fn touch_db_identity_opened_at(&self) -> CoreResult<()> {
        let connection = self.open_connection()?;
        connection
            .execute(
                "UPDATE db_identity SET last_opened_at = ?1 WHERE id = 1",
                params![Utc::now().to_rfc3339()],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }
}

impl InventoryRepository for SqliteInventoryRepository {
    fn get_sync_state(&self) -> CoreResult<Option<SyncState>> {
        let connection = self.open_connection()?;
        let row = connection
            .query_row(
                "SELECT account_id, email, display_name, committed_start_page_token, committed_generation, active_scopes_json, last_sync_status FROM sync_state LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| CoreError::Message(error.to_string()))?;

        row.map(
            |(
                account_id,
                email,
                display_name,
                committed_start_page_token,
                committed_generation,
                scopes_json,
                last_sync_status,
            )| {
                Ok(SyncState {
                    account: AccountProfile { account_id, email, display_name },
                    committed_start_page_token,
                    committed_generation,
                    active_scopes: serde_json::from_str(&scopes_json)?,
                    last_sync_status: parse_sync_status(&last_sync_status),
                })
            },
        )
        .transpose()
    }

    fn get_sync_state_for_account(&self, account_id: &str) -> CoreResult<Option<SyncState>> {
        let connection = self.open_connection()?;
        let row = connection
            .query_row(
                "SELECT account_id, email, display_name, committed_start_page_token, committed_generation, active_scopes_json, last_sync_status FROM sync_state WHERE account_id = ?1",
                params![account_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| CoreError::Message(error.to_string()))?;

        row.map(
            |(
                account_id,
                email,
                display_name,
                committed_start_page_token,
                committed_generation,
                scopes_json,
                last_sync_status,
            )| {
                Ok(SyncState {
                    account: AccountProfile { account_id, email, display_name },
                    committed_start_page_token,
                    committed_generation,
                    active_scopes: serde_json::from_str(&scopes_json)?,
                    last_sync_status: parse_sync_status(&last_sync_status),
                })
            },
        )
        .transpose()
    }

    fn load_snapshot(&self) -> CoreResult<FullSnapshot> {
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, mime_type, trashed, owned_by_me, shared, operator_can_share_manage, size, md5_checksum, modified_time, viewed_by_me_time, permissions_json, web_view_link, quota_bytes_used, quota_bytes_total FROM files ORDER BY id",
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, i64>(4)? != 0,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                    row.get::<_, Option<i64>>(14)?,
                ))
            })
            .map_err(|error| CoreError::Message(error.to_string()))?;

        let mut files = Vec::new();
        for row in rows {
            let (
                id,
                name,
                mime_type,
                trashed,
                owned_by_me,
                shared,
                operator_can_share_manage,
                size,
                md5_checksum,
                modified_time,
                viewed_by_me_time,
                permissions_json,
                web_view_link,
                quota_bytes_used,
                quota_bytes_total,
            ) = row.map_err(|error| CoreError::Message(error.to_string()))?;
            let mut parent_statement = connection
                .prepare("SELECT parent_id FROM parents WHERE file_id = ?1 ORDER BY parent_id")
                .map_err(|error| CoreError::Message(error.to_string()))?;
            let parent_rows = parent_statement
                .query_map(params![&id], |parent_row| parent_row.get::<_, String>(0))
                .map_err(|error| CoreError::Message(error.to_string()))?;
            let mut parents = Vec::new();
            for parent_row in parent_rows {
                parents.push(parent_row.map_err(|error| CoreError::Message(error.to_string()))?);
            }

            files.push(FileRecord {
                id,
                name,
                mime_type,
                parents,
                trashed,
                owned_by_me,
                shared,
                operator_can_share_manage,
                size: size.map(|value| value as u64),
                md5_checksum,
                modified_time: parse_optional_datetime(modified_time)?,
                viewed_by_me_time: parse_optional_datetime(viewed_by_me_time)?,
                permissions: serde_json::from_str::<Vec<PermissionRecord>>(&permissions_json)?,
                web_view_link,
                quota_bytes_used: quota_bytes_used.map(|value| value as u64),
                quota_bytes_total: quota_bytes_total.map(|value| value as u64),
                image_media_metadata: None,
            });
        }

        Ok(FullSnapshot { files })
    }

    fn load_inventory_items(&self) -> CoreResult<Vec<InventoryItem>> {
        Ok(build_inventory_items(&self.load_snapshot()?))
    }

    fn inspect_file(&self, id: &str) -> CoreResult<Option<InventoryItem>> {
        Ok(self.load_inventory_items()?.into_iter().find(|item| item.file.id == id))
    }

    fn append_audit_log(&self, entry: &AuditLogEntry) -> CoreResult<()> {
        let connection = self.open_connection()?;
        connection
            .execute(
                "INSERT INTO audit_log (at, command, action, file_id, permission_id, target_label, dry_run, source_file_id, backup_file_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    entry.at.to_rfc3339(),
                    &entry.command,
                    &entry.action,
                    &entry.file_id,
                    &entry.permission_id,
                    &entry.target_label,
                    if entry.dry_run { 1 } else { 0 },
                    &entry.source_file_id,
                    &entry.backup_file_id,
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }

    fn load_audit_log(&self) -> CoreResult<Vec<AuditLogEntry>> {
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT at, command, action, file_id, permission_id, target_label, dry_run, source_file_id, backup_file_id FROM audit_log ORDER BY id",
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let mut entries = Vec::new();
        for row in rows {
            let (
                at,
                command,
                action,
                file_id,
                permission_id,
                target_label,
                dry_run,
                source_file_id,
                backup_file_id,
            ) = row.map_err(|error| CoreError::Message(error.to_string()))?;
            entries.push(AuditLogEntry {
                at: DateTime::parse_from_rfc3339(&at)
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|error| CoreError::Message(error.to_string()))?,
                command,
                action,
                file_id,
                permission_id,
                target_label,
                dry_run,
                source_file_id,
                backup_file_id,
            });
        }
        Ok(entries)
    }

    fn append_revoked_share(&self, entry: &RevokedShareEntry) -> CoreResult<()> {
        let connection = self.open_connection()?;
        connection
            .execute(
                "INSERT INTO revoked_share_history (revoked_at, command, file_id, file_name, file_path, grantee, grantee_type, role, permission_id, inherited, source_folder_id, revoked_via, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    entry.at.to_rfc3339(),
                    &entry.command,
                    &entry.file_id,
                    &entry.file_name,
                    &entry.file_path,
                    &entry.grantee,
                    &entry.grantee_type,
                    &entry.role,
                    &entry.permission_id,
                    if entry.inherited { 1 } else { 0 },
                    &entry.source_folder_id,
                    &entry.revoked_via,
                    &entry.note,
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }

    fn load_revoked_shares(&self) -> CoreResult<Vec<RevokedShareEntry>> {
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT revoked_at, command, file_id, file_name, file_path, grantee, grantee_type, role, permission_id, inherited, source_folder_id, revoked_via, note FROM revoked_share_history ORDER BY id",
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(RevokedShareEntry {
                    at: parse_rfc3339_column(row.get::<_, String>(0)?)?,
                    command: row.get(1)?,
                    file_id: row.get(2)?,
                    file_name: row.get(3)?,
                    file_path: row.get(4)?,
                    grantee: row.get(5)?,
                    grantee_type: row.get(6)?,
                    role: row.get(7)?,
                    permission_id: row.get(8)?,
                    inherited: row.get::<_, i64>(9)? != 0,
                    source_folder_id: row.get(10)?,
                    revoked_via: row.get(11)?,
                    note: row.get(12)?,
                })
            })
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.map_err(|error| CoreError::Message(error.to_string()))?);
        }
        Ok(entries)
    }

    fn append_trashed_file(&self, entry: &TrashedFileEntry) -> CoreResult<()> {
        let connection = self.open_connection()?;
        connection
            .execute(
                "INSERT INTO trashed_file_history (trashed_at, recoverable_until, command, file_id, file_name, file_path, mime_type, size, md5_checksum, modified_time, trashed_via_file_id, trashed_via_path, explicitly_requested, descendant_file_count, descendant_folder_count, trash_via, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![
                    entry.at.to_rfc3339(),
                    entry.recoverable_until.as_ref().map(DateTime::<Utc>::to_rfc3339),
                    &entry.command,
                    &entry.file_id,
                    &entry.file_name,
                    &entry.file_path,
                    &entry.mime_type,
                    entry.size.map(|value| value as i64),
                    &entry.md5_checksum,
                    entry.modified_time.as_ref().map(DateTime::<Utc>::to_rfc3339),
                    &entry.trashed_via_file_id,
                    &entry.trashed_via_path,
                    if entry.explicitly_requested { 1 } else { 0 },
                    entry.descendant_file_count as i64,
                    entry.descendant_folder_count as i64,
                    &entry.trash_via,
                    &entry.note,
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }

    fn load_trashed_files(&self) -> CoreResult<Vec<TrashedFileEntry>> {
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT trashed_at, recoverable_until, command, file_id, file_name, file_path, mime_type, size, md5_checksum, modified_time, trashed_via_file_id, trashed_via_path, explicitly_requested, descendant_file_count, descendant_folder_count, trash_via, note FROM trashed_file_history ORDER BY id",
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, String>(15)?,
                    row.get::<_, Option<String>>(16)?,
                ))
            })
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let mut entries = Vec::new();
        for row in rows {
            let (
                trashed_at,
                recoverable_until,
                command,
                file_id,
                file_name,
                file_path,
                mime_type,
                size,
                md5_checksum,
                modified_time,
                trashed_via_file_id,
                trashed_via_path,
                explicitly_requested,
                descendant_file_count,
                descendant_folder_count,
                trash_via,
                note,
            ) = row.map_err(|error| CoreError::Message(error.to_string()))?;
            entries.push(TrashedFileEntry {
                at: parse_datetime(&trashed_at)?,
                recoverable_until: parse_optional_datetime(recoverable_until)?,
                command,
                file_id,
                file_name,
                file_path,
                mime_type,
                size: size.map(|value| value as u64),
                md5_checksum,
                modified_time: parse_optional_datetime(modified_time)?,
                trashed_via_file_id,
                trashed_via_path,
                explicitly_requested: explicitly_requested != 0,
                descendant_file_count: descendant_file_count as usize,
                descendant_folder_count: descendant_folder_count as usize,
                trash_via,
                note,
            });
        }
        Ok(entries)
    }

    fn append_moved_file(&self, entry: &MovedFileEntry) -> CoreResult<()> {
        let connection = self.open_connection()?;
        let from_parent_ids_json = serde_json::to_string(&entry.from_parent_ids)
            .map_err(|error| CoreError::Message(error.to_string()))?;
        connection
            .execute(
                "INSERT INTO moved_file_history (moved_at, command, status, file_id, file_name, file_path, mime_type, from_parent_ids_json, from_path, to_parent_id, to_path, move_via, note)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    entry.at.to_rfc3339(),
                    &entry.command,
                    &entry.status,
                    &entry.file_id,
                    &entry.file_name,
                    &entry.file_path,
                    &entry.mime_type,
                    &from_parent_ids_json,
                    &entry.from_path,
                    &entry.to_parent_id,
                    &entry.to_path,
                    &entry.move_via,
                    &entry.note,
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }

    fn load_moved_files(&self) -> CoreResult<Vec<MovedFileEntry>> {
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT moved_at, command, status, file_id, file_name, file_path, mime_type, from_parent_ids_json, from_path, to_parent_id, to_path, move_via, note FROM moved_file_history ORDER BY id",
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<String>>(12)?,
                ))
            })
            .map_err(|error| CoreError::Message(error.to_string()))?;
        let mut entries = Vec::new();
        for row in rows {
            let (
                moved_at,
                command,
                status,
                file_id,
                file_name,
                file_path,
                mime_type,
                from_parent_ids_json,
                from_path,
                to_parent_id,
                to_path,
                move_via,
                note,
            ) = row.map_err(|error| CoreError::Message(error.to_string()))?;
            entries.push(MovedFileEntry {
                at: parse_datetime(&moved_at)?,
                command,
                status,
                file_id,
                file_name,
                file_path,
                mime_type,
                from_parent_ids: serde_json::from_str(&from_parent_ids_json)?,
                from_path,
                to_parent_id,
                to_path,
                move_via,
                note,
            });
        }
        Ok(entries)
    }

    fn begin_sync_run(
        &self,
        account: &AccountProfile,
        _active_scopes: &[DriveScope],
        mode: SyncMode,
        source_page_token: Option<&str>,
    ) -> CoreResult<SyncRun> {
        let mut connection = self.open_connection()?;
        let transaction =
            connection.transaction().map_err(|error| CoreError::Message(error.to_string()))?;
        transaction
            .execute(
                "UPDATE sync_runs SET status = 'failed', completed_at = ?1, error_text = COALESCE(error_text, 'superseded by a new sync run') WHERE status = 'in_progress'",
                params![Utc::now().to_rfc3339()],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;

        let current_generation = transaction
            .query_row(
                "SELECT committed_generation FROM sync_state WHERE account_id = ?1",
                params![&account.account_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| CoreError::Message(error.to_string()))?
            .unwrap_or(0);
        let generation = current_generation + 1;
        let started_at = Utc::now();
        let run_id = format!("sync-{}-{generation}", started_at.timestamp_millis());

        transaction
            .execute(
                "INSERT INTO sync_runs (run_id, account_id, mode, status, source_page_token, generation, started_at) VALUES (?1, ?2, ?3, 'in_progress', ?4, ?5, ?6)",
                params![
                    &run_id,
                    &account.account_id,
                    mode.as_str(),
                    source_page_token,
                    generation,
                    started_at.to_rfc3339(),
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        transaction.commit().map_err(|error| CoreError::Message(error.to_string()))?;

        Ok(SyncRun {
            run_id,
            mode,
            generation,
            source_page_token: source_page_token.map(ToOwned::to_owned),
            started_at,
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
        let mut connection = self.open_connection()?;
        let transaction =
            connection.transaction().map_err(|error| CoreError::Message(error.to_string()))?;
        let synced_at = Utc::now().to_rfc3339();
        let path_entries = build_path_entries(snapshot);
        let scopes_json = serde_json::to_string(active_scopes)
            .map_err(|error| CoreError::Message(error.to_string()))?;

        transaction
            .execute("DELETE FROM path_cache", [])
            .map_err(|error| CoreError::Message(error.to_string()))?;
        transaction
            .execute("DELETE FROM parents", [])
            .map_err(|error| CoreError::Message(error.to_string()))?;
        transaction
            .execute("DELETE FROM files", [])
            .map_err(|error| CoreError::Message(error.to_string()))?;

        for file in &snapshot.files {
            let permissions_json = serde_json::to_string(&file.permissions)
                .map_err(|error| CoreError::Message(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO files (id, name, mime_type, trashed, owned_by_me, shared, operator_can_share_manage, size, md5_checksum, modified_time, viewed_by_me_time, permissions_json, web_view_link, quota_bytes_used, quota_bytes_total, generation, synced_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                    params![
                        &file.id,
                        &file.name,
                        &file.mime_type,
                        if file.trashed { 1 } else { 0 },
                        if file.owned_by_me { 1 } else { 0 },
                        if file.shared { 1 } else { 0 },
                        if file.operator_can_share_manage { 1 } else { 0 },
                        file.size.map(|value| value as i64),
                        &file.md5_checksum,
                        file.modified_time.as_ref().map(DateTime::<Utc>::to_rfc3339),
                        file.viewed_by_me_time.as_ref().map(DateTime::<Utc>::to_rfc3339),
                        &permissions_json,
                        &file.web_view_link,
                        file.quota_bytes_used.map(|value| value as i64),
                        file.quota_bytes_total.map(|value| value as i64),
                        run.generation,
                        &synced_at,
                    ],
                )
                .map_err(|error| CoreError::Message(error.to_string()))?;

            for parent_id in &file.parents {
                transaction
                    .execute(
                        "INSERT INTO parents (file_id, parent_id) VALUES (?1, ?2)",
                        params![&file.id, parent_id],
                    )
                    .map_err(|error| CoreError::Message(error.to_string()))?;
            }
        }

        for entry in &path_entries {
            let all_paths_json = serde_json::to_string(&entry.all_paths)
                .map_err(|error| CoreError::Message(error.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO path_cache (file_id, primary_path, all_paths_json, depth, path_state, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        &entry.file_id,
                        &entry.primary_path,
                        &all_paths_json,
                        entry.depth as i64,
                        entry.path_state.as_str(),
                        &synced_at,
                    ],
                )
                .map_err(|error| CoreError::Message(error.to_string()))?;
        }

        transaction
            .execute(
                "INSERT INTO sync_state (account_id, email, display_name, committed_start_page_token, committed_generation, active_scopes_json, last_sync_status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'committed')
                 ON CONFLICT(account_id) DO UPDATE SET
                    email = excluded.email,
                    display_name = excluded.display_name,
                    committed_start_page_token = excluded.committed_start_page_token,
                    committed_generation = excluded.committed_generation,
                    active_scopes_json = excluded.active_scopes_json,
                    last_sync_status = excluded.last_sync_status",
                params![
                    &account.account_id,
                    &account.email,
                    &account.display_name,
                    committed_page_token,
                    run.generation,
                    &scopes_json,
                ],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;

        transaction
            .execute(
                "UPDATE sync_runs SET status = 'committed', completed_at = ?1, committed_page_token = ?2, error_text = NULL WHERE run_id = ?3",
                params![Utc::now().to_rfc3339(), committed_page_token, &run.run_id],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        transaction.commit().map_err(|error| CoreError::Message(error.to_string()))?;

        Ok(SyncSummary {
            mode: run.mode,
            added: stats.added,
            updated: stats.updated,
            removed: stats.removed,
            committed_page_token: committed_page_token.to_string(),
            generation: run.generation,
            file_count: snapshot.files.len(),
            path_count: path_entries.len(),
        })
    }

    fn mark_sync_failed(&self, run: &SyncRun, message: &str) -> CoreResult<()> {
        let connection = self.open_connection()?;
        connection
            .execute(
                "UPDATE sync_runs SET status = 'failed', completed_at = ?1, error_text = ?2 WHERE run_id = ?3",
                params![Utc::now().to_rfc3339(), message, &run.run_id],
            )
            .map_err(|error| CoreError::Message(error.to_string()))?;
        Ok(())
    }
}

fn parse_sync_status(raw: &str) -> SyncStatus {
    match raw {
        "in_progress" => SyncStatus::InProgress,
        "committed" => SyncStatus::Committed,
        "failed" => SyncStatus::Failed,
        _ => SyncStatus::Never,
    }
}

fn parse_path_state(raw: &str) -> PathState {
    match raw {
        "multi_parent" => PathState::MultiParent,
        "orphaned" => PathState::Orphaned,
        _ => PathState::Resolved,
    }
}

fn parse_optional_datetime(raw: Option<String>) -> CoreResult<Option<DateTime<Utc>>> {
    raw.map(|value| {
        DateTime::parse_from_rfc3339(&value)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|error| CoreError::Message(error.to_string()))
    })
    .transpose()
}

fn parse_datetime(raw: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|error| CoreError::Message(error.to_string()))
}

fn query_count(connection: &Connection, sql: &str) -> CoreResult<usize> {
    connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|count| count as usize)
        .map_err(|error| CoreError::Message(error.to_string()))
}

fn parse_rfc3339_column(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value).map(|value| value.with_timezone(&Utc)).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn query_pragma_u64(connection: &Connection, pragma: &str) -> CoreResult<u64> {
    connection
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get::<_, i64>(0))
        .map(|value| value as u64)
        .map_err(|error| CoreError::Message(error.to_string()))
}

#[cfg(test)]
mod lib_tests;
