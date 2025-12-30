pub mod models;
pub mod clients;
pub mod pipeline;
pub mod indexing;
pub mod storage;

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn setup_db(url: &str) -> Result<SqlitePool> {
    let pool = SqlitePool::connect(url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
