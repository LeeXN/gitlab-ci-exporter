use sqlx::{sqlite::SqlitePoolOptions, Pool, Sqlite};
use anyhow::Result;

const INIT_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS pipelines (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL,
    project_name TEXT NOT NULL,
    project_full_path TEXT NOT NULL,
    ref_name TEXT NOT NULL,
    user_name TEXT,
    sha TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    finished_at INTEGER,
    duration INTEGER,
    web_url TEXT,
    UNIQUE(id)
);
CREATE TABLE IF NOT EXISTS poll_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_poll_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS daily_stats (
    date TEXT NOT NULL,
    project_id INTEGER NOT NULL,
    project_name TEXT NOT NULL,
    status TEXT NOT NULL,
    count INTEGER DEFAULT 0,
    total_duration INTEGER DEFAULT 0,
    count_with_duration INTEGER DEFAULT 0,
    PRIMARY KEY (date, project_id, status)
);
CREATE INDEX IF NOT EXISTS idx_query ON pipelines(project_name, status, created_at);
CREATE INDEX IF NOT EXISTS idx_status_created ON pipelines(status, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_project_created ON pipelines(project_name, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_watermark ON pipelines(finished_at);

"#;

pub async fn init_db() -> Result<Pool<Sqlite>> {
    // Re-connecting to a file
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("sqlite:pipelines.db?mode=rwc").await?;

    sqlx::query(INIT_SQL).execute(&pool).await?;
    // Ensure `count_with_duration` column exists (migration for older DBs)
    let has_col: Option<i64> = sqlx::query_scalar("SELECT 1 FROM pragma_table_info('daily_stats') WHERE name = 'count_with_duration' LIMIT 1")
        .fetch_optional(&pool)
        .await?;
    if has_col.is_none() {
        // Add the column with default 0
        sqlx::query("ALTER TABLE daily_stats ADD COLUMN count_with_duration INTEGER DEFAULT 0;")
            .execute(&pool)
            .await?;
    }
    let current_time = chrono::Utc::now().timestamp();
    if get_last_poll(&pool).await?.is_none() {
        set_last_poll(&pool, current_time).await?;
    }
    Ok(pool)
}

pub async fn get_last_poll(pool: &Pool<Sqlite>) -> Result<Option<i64>> {
    let row: Option<i64> = sqlx::query_scalar("SELECT last_poll_at FROM poll_state WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

pub async fn set_last_poll(pool: &Pool<Sqlite>, ts: i64) -> Result<()> {
    sqlx::query("INSERT INTO poll_state (id, last_poll_at) VALUES (1, ?) ON CONFLICT(id) DO UPDATE SET last_poll_at = excluded.last_poll_at")
        .bind(ts)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn backfill_daily_stats(pool: &Pool<Sqlite>) -> Result<()> {
    // Aggregate pipelines into daily_stats
    // Use date(created_at, 'unixepoch') to get YYYY-MM-DD
    let mut tx = pool.begin().await?;

    // Insert aggregated counts and total durations, upsert on conflict
    let q = r#"
    INSERT INTO daily_stats (date, project_id, project_name, status, count, total_duration, count_with_duration)
    SELECT date(created_at, 'unixepoch') as date,
           project_id,
           project_name,
           status,
           COUNT(*) as count,
           COALESCE(SUM(duration),0) as total_duration,
           SUM(CASE WHEN duration IS NOT NULL THEN 1 ELSE 0 END) as count_with_duration
    FROM pipelines
    GROUP BY date, project_id, project_name, status
    ON CONFLICT(date, project_id, status) DO UPDATE SET
        count = excluded.count,
        total_duration = excluded.total_duration,
        count_with_duration = excluded.count_with_duration,
        project_name = excluded.project_name
    "#;

    sqlx::query(q).execute(&mut *tx).await?;

    tx.commit().await?;
    Ok(())
}