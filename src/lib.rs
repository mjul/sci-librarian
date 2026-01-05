pub mod clients;
pub mod indexing;
pub mod models;
pub mod pipeline;
pub mod storage;

use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;

pub async fn setup_db(url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
    let pool = SqlitePool::connect_with(options).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}
