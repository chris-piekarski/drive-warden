-- Durable audit trail for revoked sharing permissions.
-- Unlike `files.permissions_json` (overwritten on every sync) this table is
-- append-only, so operators retain the full history of who lost access to what.
CREATE TABLE IF NOT EXISTS revoked_share_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    revoked_at TEXT NOT NULL,
    command TEXT NOT NULL,
    file_id TEXT NOT NULL,
    file_name TEXT NOT NULL DEFAULT '',
    file_path TEXT NOT NULL DEFAULT '',
    grantee TEXT NOT NULL,
    grantee_type TEXT NOT NULL DEFAULT '',
    role TEXT NOT NULL DEFAULT '',
    permission_id TEXT NOT NULL,
    inherited INTEGER NOT NULL DEFAULT 0,
    source_folder_id TEXT,
    revoked_via TEXT NOT NULL DEFAULT 'tool',
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_revoked_share_grantee ON revoked_share_history(grantee);
CREATE INDEX IF NOT EXISTS idx_revoked_share_file ON revoked_share_history(file_id);
