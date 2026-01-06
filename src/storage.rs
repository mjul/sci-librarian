use crate::models::{DropboxId, FileHash, FileRecord, FileStatus};
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;

pub struct Storage {
    pool: SqlitePool,
}

impl Storage {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_file(
        &self,
        id: &DropboxId,
        file_name: &str,
        hash: &FileHash,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO files (dropbox_id, file_name, content_hash, status, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(dropbox_id) DO UPDATE SET
                file_name = excluded.file_name,
                content_hash = excluded.content_hash,
                status = CASE 
                    WHEN files.content_hash != excluded.content_hash THEN ?4
                    ELSE files.status
                END,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&id.0)
        .bind(file_name)
        .bind(&hash.0)
        .bind(FileStatus::Pending)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_pending_files(&self, limit: i64) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as::<_, FileRecord>(
            r#"
            SELECT 
                dropbox_id,
                file_name,
                content_hash,
                status,
                title,
                authors,
                summary,
                target_path,
                last_error,
                updated_at
            FROM files
            WHERE status = 'PENDING'
            ORDER BY updated_at DESC
            LIMIT ?1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(records)
    }

    pub async fn update_status(&self, id: &DropboxId, status: FileStatus) -> Result<()> {
        sqlx::query("UPDATE files SET status = ?1, updated_at = ?2 WHERE dropbox_id = ?3")
            .bind(status)
            .bind(Utc::now())
            .bind(&id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_files_in_folder(&self, folder: &str) -> Result<Vec<FileRecord>> {
        let records = sqlx::query_as::<_, FileRecord>(
            r#"
            SELECT 
                dropbox_id,
                file_name,
                content_hash,
                status,
                title,
                authors,
                summary,
                target_path,
                last_error,
                updated_at
            FROM files
            WHERE target_path LIKE ?1
            ORDER BY title ASC
            "#,
        )
        .bind(format!("%{}%", folder)) // Simple match for now
        .fetch_all(&self.pool)
        .await?;
        Ok(records)
    }
}
