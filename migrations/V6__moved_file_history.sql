CREATE TABLE IF NOT EXISTS moved_file_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    moved_at TEXT NOT NULL,
    command TEXT NOT NULL,
    status TEXT NOT NULL,
    file_id TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_path TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    from_parent_ids_json TEXT NOT NULL,
    from_path TEXT NOT NULL,
    to_parent_id TEXT NOT NULL,
    to_path TEXT NOT NULL,
    move_via TEXT NOT NULL,
    note TEXT
);

CREATE INDEX IF NOT EXISTS idx_moved_file_history_file_id
    ON moved_file_history(file_id);

CREATE INDEX IF NOT EXISTS idx_moved_file_history_moved_at
    ON moved_file_history(moved_at);
