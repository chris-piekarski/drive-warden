PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS db_identity (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    db_instance_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_opened_at TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS remote_sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    generation INTEGER NOT NULL DEFAULT 0,
    last_pushed_at TEXT,
    last_pulled_at TEXT,
    last_remote_file_id TEXT,
    last_manifest_sha256 TEXT,
    last_manifest_uploaded_at TEXT,
    last_remote_byte_len INTEGER,
    last_source_label TEXT
);

INSERT OR IGNORE INTO db_identity (
    id,
    db_instance_id,
    created_at,
    last_opened_at,
    schema_version
) VALUES (
    1,
    lower(hex(randomblob(16))),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    1
);

INSERT OR IGNORE INTO remote_sync_state (id, generation) VALUES (1, 0);
