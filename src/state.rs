use crate::config::Config;
use crate::gitlab_types::ProjectInfo;
use crate::gitlab_graphql::GitlabGraphqlClient;
use gitlab::AsyncGitlab;
use sqlx::SqlitePool;
use moka::future::Cache;
use serde_json::Value as JsonValue;
use std::sync::{Arc, RwLock};
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub gitlab_client: Arc<AsyncGitlab>,
    pub graphql_client: Arc<GitlabGraphqlClient>,
    pub config: Arc<Config>,
    pub monitored_projects: Arc<RwLock<Vec<ProjectInfo>>>,
    pub refresh_notify: Arc<Notify>,
    #[allow(dead_code)]
    pub is_fresh_install: bool,
    pub cache: Cache<String, JsonValue>,
}
