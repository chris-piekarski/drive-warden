-- Durable audit trail for files moved to Google Drive trash.
-- `files`, `parents`, and `path_cache` are active snapshot tables rewritten by
-- sync. This table is append-only so operators retain recovery timelines after
-- trashed files disappear from the active inventory.
CREATE TABLE IF NOT EXISTS trashed_file_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    trashed_at TEXT NOT NULL,
    recoverable_until TEXT,
    command TEXT NOT NULL,
    file_id TEXT NOT NULL,
    file_name TEXT NOT NULL DEFAULT '',
    file_path TEXT NOT NULL DEFAULT '',
    mime_type TEXT NOT NULL DEFAULT '',
    size INTEGER,
    md5_checksum TEXT,
    modified_time TEXT,
    trashed_via_file_id TEXT,
    trashed_via_path TEXT,
    explicitly_requested INTEGER NOT NULL DEFAULT 0,
    descendant_file_count INTEGER NOT NULL DEFAULT 0,
    descendant_folder_count INTEGER NOT NULL DEFAULT 0,
    trash_via TEXT NOT NULL DEFAULT 'tool',
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_trashed_file_history_file ON trashed_file_history(file_id);
CREATE INDEX IF NOT EXISTS idx_trashed_file_history_path ON trashed_file_history(file_path);
CREATE INDEX IF NOT EXISTS idx_trashed_file_history_trashed_at ON trashed_file_history(trashed_at);
CREATE INDEX IF NOT EXISTS idx_trashed_file_history_recoverable_until ON trashed_file_history(recoverable_until);
