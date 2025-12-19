mod api;
mod config;
mod db;
mod gitlab_ops;
mod gitlab_graphql;
mod models;
mod gitlab_types;
mod monitor;
mod state;

use crate::config::Config;
use crate::state::AppState;
use anyhow::Result;
use gitlab::GitlabBuilder;
use std::sync::{Arc, RwLock};
use tracing::info;
use moka::future::Cache;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    // Load Config
    let config = Config::new().expect("Failed to load config");
    let config = Arc::new(config);

    // Initialize DB
    let db = db::init_db().await.expect("Failed to initialize database");

    // Record service start time as initial poll watermark
    if let Err(e) = crate::db::set_last_poll(&db, chrono::Utc::now().timestamp()).await {
        tracing::warn!("Failed to set initial poll watermark: {}", e);
    }

    // Check if this is a fresh install (no pipelines)
    let pipeline_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pipelines")
        .fetch_one(&db)
        .await
        .unwrap_or(0);
    let is_fresh_install = pipeline_count == 0;
    if is_fresh_install {
        info!("Fresh install detected. Will perform initial backfill for all projects.");
    }

    let host = config.gitlab.url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
        
    let gitlab_client = GitlabBuilder::new(host, config.gitlab.token.clone())
        .build_async()
        .await
        .expect("Failed to create GitLab client");
    let gitlab_client = Arc::new(gitlab_client);

    // Initialize GraphQL Client
    let graphql_client = crate::gitlab_graphql::GitlabGraphqlClient::new(
        config.gitlab.url.clone(),
        config.gitlab.token.clone(),
        config.gitlab.timeout_seconds.unwrap_or(30),
        config.gitlab.skip_invalid_certs.unwrap_or(false),
    );
    let graphql_client = Arc::new(graphql_client);

    let ttl = config.poller.ttl_seconds.unwrap_or(600) as u64;
    let capacity = config.poller.capacity.unwrap_or(10_000) as u64;

    // Create AppState
    let state = AppState {
        db,
        gitlab_client,
        graphql_client,
        config: config.clone(),
        monitored_projects: Arc::new(RwLock::new(Vec::new())),
        refresh_notify: Arc::new(tokio::sync::Notify::new()),
        is_fresh_install,
        cache: Cache::builder()
            .time_to_live(std::time::Duration::from_secs(ttl))
            .max_capacity(capacity)
            .build(),
    };

    // Perform initial backfill if needed (BLOCKING)
    if is_fresh_install {
        monitor::perform_initial_backfill(state.clone()).await;
        // After initial backfill, asynchronously backfill usernames (do not block server start)
        let username_state = state.clone();
        tokio::spawn(async move {
            monitor::backfill_usernames(username_state).await;
        });
    }

    // If not fresh install, start username backfill in background as well
    if !is_fresh_install {
        let username_state = state.clone();
        tokio::spawn(async move {
            monitor::backfill_usernames(username_state).await;
        });
    }

    // Ensure daily_stats is populated on startup; if empty, run backfill
    let daily_stats_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_stats")
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
    if daily_stats_count == 0 {
        info!("daily_stats empty — running backfill_daily_stats on startup");
        if let Err(e) = crate::db::backfill_daily_stats(&state.db).await {
            tracing::error!("daily_stats backfill on startup failed: {}", e);
        } else {
            info!("daily_stats backfill completed");
        }
    }

    // Start Monitor Loop in background
    let monitor_state = state.clone();
    tokio::spawn(async move {
        monitor::start_monitor_loop(monitor_state).await;
    });

    // Start Web Server
    let app = api::app_router(state);
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    
    info!("Server running on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}
