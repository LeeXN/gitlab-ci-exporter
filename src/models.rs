use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Pipeline {
    pub id: i64,
    pub project_id: i64,
    pub project_name: String,
    pub project_full_path: String,
    pub ref_name: String,
    pub sha: String,
    pub user_name: String,
    pub status: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub duration: Option<i64>,
    pub web_url: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DailyStat {
    pub date: String,
    pub status: String,
    pub count: i64,
}
