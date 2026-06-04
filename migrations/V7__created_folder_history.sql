CREATE TABLE IF NOT EXISTS created_folder_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at TEXT NOT NULL,
    command TEXT NOT NULL,
    status TEXT NOT NULL,
    folder_id TEXT NOT NULL,
    folder_name TEXT NOT NULL,
    folder_path TEXT NOT NULL,
    parent_id TEXT NOT NULL,
    parent_path TEXT NOT NULL,
    provision_path TEXT NOT NULL,
    create_via TEXT NOT NULL,
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_created_folder_history_folder_id
    ON created_folder_history(folder_id);

CREATE INDEX IF NOT EXISTS idx_created_folder_history_created_at
    ON created_folder_history(created_at);

ALTER TABLE moved_file_history ADD COLUMN moved_via_file_id TEXT;
ALTER TABLE moved_file_history ADD COLUMN moved_via_path TEXT;
ALTER TABLE moved_file_history ADD COLUMN explicitly_requested INTEGER NOT NULL DEFAULT 1;
