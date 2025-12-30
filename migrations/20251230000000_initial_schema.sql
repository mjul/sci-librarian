CREATE TABLE files (
    dropbox_id TEXT PRIMARY KEY,
    content_hash TEXT NOT NULL,
    status TEXT NOT NULL,       -- 'PENDING', 'DOWNLOADED', 'PROCESSED', 'ARCHIVED', 'ERROR', 'SKIPPED'
    title TEXT,
    authors TEXT,               -- JSON array
    summary TEXT,               -- Cached for README generation
    target_path TEXT,           -- For indexing lookups (comma separated or JSON if multiple)
    last_error TEXT,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_folder ON files(target_path);
