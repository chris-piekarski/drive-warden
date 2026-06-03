PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sync_state (
    account_id TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    display_name TEXT,
    committed_start_page_token TEXT,
    committed_generation INTEGER NOT NULL DEFAULT 0,
    active_scopes_json TEXT NOT NULL DEFAULT '[]',
    last_sync_status TEXT NOT NULL DEFAULT 'never'
);

CREATE TABLE IF NOT EXISTS sync_runs (
    run_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL,
    source_page_token TEXT,
    generation INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    committed_page_token TEXT,
    error_text TEXT
);

CREATE TABLE IF NOT EXISTS files (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    trashed INTEGER NOT NULL DEFAULT 0,
    owned_by_me INTEGER NOT NULL DEFAULT 0,
    shared INTEGER NOT NULL DEFAULT 0,
    operator_can_share_manage INTEGER NOT NULL DEFAULT 0,
    size INTEGER,
    md5_checksum TEXT,
    modified_time TEXT,
    viewed_by_me_time TEXT,
    permissions_json TEXT NOT NULL DEFAULT '[]',
    web_view_link TEXT,
    quota_bytes_used INTEGER,
    quota_bytes_total INTEGER,
    generation INTEGER NOT NULL,
    synced_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS parents (
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    parent_id TEXT NOT NULL,
    PRIMARY KEY (file_id, parent_id)
);

CREATE TABLE IF NOT EXISTS path_cache (
    file_id TEXT PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    primary_path TEXT NOT NULL,
    all_paths_json TEXT NOT NULL,
    depth INTEGER NOT NULL DEFAULT 0,
    path_state TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    command TEXT NOT NULL,
    action TEXT NOT NULL,
    file_id TEXT NOT NULL,
    permission_id TEXT NOT NULL,
    target_label TEXT NOT NULL,
    dry_run INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sync_runs_status ON sync_runs(status, started_at);
CREATE INDEX IF NOT EXISTS idx_parents_parent ON parents(parent_id);
CREATE INDEX IF NOT EXISTS idx_path_cache_primary_path ON path_cache(primary_path);
