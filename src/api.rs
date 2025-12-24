use crate::models::{DailyStat, Pipeline};
use crate::state::AppState;
use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use chrono::TimeZone;

#[derive(Deserialize, Clone, Debug)]
pub struct PipelineFilter {
    project_name: Option<String>,
    ref_name: Option<String>,
    exclude_projects: Option<String>,
    status: Option<String>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectStat {
    pub project_name: String,
    pub count: i64,
    pub avg_duration: f64,
    pub last_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SummaryStat {
    pub total_count: i64,
    pub avg_duration: f64,
    pub success_rate: f64,
}

#[derive(Serialize)]
pub struct PipelineResponse {
    pub id: i64,
    pub project_id: i64,
    pub project_name: String,
    pub project_full_path: String,
    pub ref_name: String,
    pub sha: String,
    pub user_name: String,
    pub status: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub duration: Option<i64>,
    pub web_url: Option<String>,
}

pub fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/api/pipelines", get(list_pipelines))
        .route("/api/refresh_daily_stats", post(trigger_refresh_daily_stats))
        .route("/api/stats/trend", get(get_stats_trend))
        .route("/api/stats/projects", get(get_project_stats))
        .route("/api/stats/summary", get(get_summary_stats))
        .route("/api/projects", get(list_projects))
        .route("/api/refs", get(list_refs))
        .with_state(state)
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FailProject {
    pub project_name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BucketCount {
    pub bucket: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectHealth {
    pub project_name: String,
    pub success_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailedJobLog {
    pub time: String,
    pub level: String,
    pub project: String,
    pub message: String,
    pub pipeline_id: i64,
}

async fn trigger_refresh_daily_stats(State(state): State<AppState>) -> Json<&'static str> {
    match crate::db::backfill_daily_stats(&state.db).await {
        Ok(_) => Json("daily_stats backfill triggered/completed"),
        Err(e) => {
            tracing::error!("daily_stats backfill failed: {}", e);
            Json("daily_stats backfill failed")
        }
    }
}

async fn get_project_stats(
    State(state): State<AppState>,
    Query(filter): Query<PipelineFilter>,
) -> Json<Vec<ProjectStat>> {
    let use_fast_path = filter.ref_name.as_deref().unwrap_or("All") == "All";

    // Build a cache key from filters
    let key = format!("projects:{:?}:{:?}:{:?}:{:?}:{:?}",
        filter.project_name.as_deref().unwrap_or("All"),
        filter.ref_name.as_deref().unwrap_or("All"),
        filter.exclude_projects.as_deref().unwrap_or(""),
        filter.from_ts,
        filter.to_ts,
    );

    // Attempt to get cached value
    if let Some(cached) = state.cache.get(&key) {
        if let Ok(v) = serde_json::from_value::<Vec<ProjectStat>>(cached.clone()) {
            return Json(v);
        }
    }

    let mut query_builder = if use_fast_path {
        sqlx::QueryBuilder::new(
            r#"
            SELECT 
                project_full_path as project_name, 
                project_full_path,
                SUM(count) as count, 
                    COALESCE(CAST(SUM(total_duration) AS REAL) / NULLIF(SUM(count_with_duration), 0), 0) as avg_duration,
                (SELECT status FROM pipelines p2 WHERE p2.project_full_path = daily_stats.project_full_path ORDER BY created_at DESC LIMIT 1) as last_status
            FROM daily_stats 
            WHERE 1=1
            "#
        )
    } else {
        sqlx::QueryBuilder::new(
            r#"
            SELECT 
                project_name, 
                project_full_path,
                COUNT(*) as count, 
                AVG(duration) as avg_duration,
                (SELECT status FROM pipelines p2 WHERE p2.project_full_path = pipelines.project_full_path ORDER BY created_at DESC LIMIT 1) as last_status
            FROM pipelines 
            WHERE 1=1
            "#
        )
    };

    if let Some(p) = &filter.project_name {
            if p != "All" && !p.is_empty() {
            if p.contains(',') {
                let projects: Vec<&str> = p.split(',').map(|s| s.trim()).collect();
                if !projects.is_empty() {
                    query_builder.push(" AND project_full_path IN (");
                    let mut separated = query_builder.separated(", ");
                    for proj in projects {
                        separated.push_bind(proj);
                    }
                    separated.push_unseparated(") ");
                }
            } else {
                query_builder.push(" AND project_full_path = ");
                query_builder.push_bind(p);
            }
        }
    }
    
    if !use_fast_path {
        if let Some(r) = &filter.ref_name {
            if r != "All" && !r.is_empty() {
                if r.contains(',') {
                    let refs: Vec<&str> = r.split(',').map(|s| s.trim()).collect();
                    if !refs.is_empty() {
                        query_builder.push(" AND ref_name IN (");
                        let mut separated = query_builder.separated(", ");
                        for rv in refs {
                            separated.push_bind(rv);
                        }
                        separated.push_unseparated(") ");
                    }
                } else {
                    query_builder.push(" AND ref_name = ");
                    query_builder.push_bind(r);
                }
            }
        }
    }
    
    if let Some(ex) = &filter.exclude_projects {
        if !ex.is_empty() {
            let projects: Vec<&str> = ex.split(',').collect();
            if !projects.is_empty() {
                query_builder.push(" AND project_full_path NOT IN (");
                let mut separated = query_builder.separated(", ");
                for p in projects {
                    separated.push_bind(p);
                }
                separated.push_unseparated(") ");
            }
        }
    }

    if use_fast_path {
        if let Some(ts) = filter.from_ts {
            query_builder.push(" AND date >= date(");
            query_builder.push_bind(ts);
            query_builder.push(", 'unixepoch')");
        }
        if let Some(ts) = filter.to_ts {
            query_builder.push(" AND date <= date(");
            query_builder.push_bind(ts);
            query_builder.push(", 'unixepoch')");
        }
    } else {
        if let Some(ts) = filter.from_ts {
            query_builder.push(" AND created_at >= ");
            query_builder.push_bind(ts);
        }
        if let Some(ts) = filter.to_ts {
            query_builder.push(" AND created_at <= ");
            query_builder.push_bind(ts);
        }
    }

    query_builder.push(" GROUP BY project_full_path ORDER BY avg_duration ASC");

    let query = query_builder.build_query_as::<ProjectStat>();
    let stats = query.fetch_all(&state.db).await.unwrap_or_default();

    // insert into cache
    if let Ok(val) = serde_json::to_value(&stats) {
        let _ = state.cache.insert(key, val);
    }

    Json(stats)
}


async fn get_summary_stats(
    State(state): State<AppState>,
    Query(filter): Query<PipelineFilter>,
) -> Json<SummaryStat> {
    let use_fast_path = filter.ref_name.as_deref().unwrap_or("All") == "All";

    let key = format!("summary:{:?}:{:?}:{:?}:{:?}:{:?}",
        filter.project_name.as_deref().unwrap_or("All"),
        filter.ref_name.as_deref().unwrap_or("All"),
        filter.exclude_projects.as_deref().unwrap_or(""),
        filter.from_ts,
        filter.to_ts,
    );

    if let Some(cached) = state.cache.get(&key) {
        if let Ok(v) = serde_json::from_value::<SummaryStat>(cached.clone()) {
            return Json(v);
        }
    }

    let mut query_builder = if use_fast_path {
        sqlx::QueryBuilder::new(
            r#"
            SELECT 
                SUM(count) as total_count, 
                COALESCE(CAST(SUM(total_duration) AS REAL) / NULLIF(SUM(count_with_duration), 0), 0) as avg_duration,
                COALESCE(SUM(CASE WHEN status = 'success' THEN count ELSE 0 END) * 100.0 / SUM(count), 0) as success_rate
            FROM daily_stats 
            WHERE 1=1
            "#
        )
    } else {
        sqlx::QueryBuilder::new(
            r#"
            SELECT 
                COUNT(*) as total_count, 
                COALESCE(AVG(duration), 0) as avg_duration,
                COALESCE(SUM(CASE WHEN status = 'success' THEN 1 ELSE 0 END) * 100.0 / COUNT(*), 0) as success_rate
            FROM pipelines 
            WHERE 1=1
            "#
        )
    };

    if let Some(p) = &filter.project_name {
        if p != "All" && !p.is_empty() {
            if p.contains(',') {
                let projects: Vec<&str> = p.split(',').map(|s| s.trim()).collect();
                if !projects.is_empty() {
                    query_builder.push(" AND project_full_path IN (");
                    let mut separated = query_builder.separated(", ");
                    for proj in projects {
                        separated.push_bind(proj);
                    }
                    separated.push_unseparated(") ");
                }
            } else {
                query_builder.push(" AND project_full_path = ");
                query_builder.push_bind(p);
            }
        }
    }
    
    if !use_fast_path {
        if let Some(r) = &filter.ref_name {
            if r != "All" && !r.is_empty() {
                if r.contains(',') {
                    let refs: Vec<&str> = r.split(',').map(|s| s.trim()).collect();
                    if !refs.is_empty() {
                        query_builder.push(" AND ref_name IN (");
                        let mut separated = query_builder.separated(", ");
                        for rv in refs {
                            separated.push_bind(rv);
                        }
                        separated.push_unseparated(") ");
                    }
                } else {
                    query_builder.push(" AND ref_name = ");
                    query_builder.push_bind(r);
                }
            }
        }
    }
    
    if let Some(ex) = &filter.exclude_projects {
        if !ex.is_empty() {
            let projects: Vec<&str> = ex.split(',').collect();
            if !projects.is_empty() {
                query_builder.push(" AND project_full_path NOT IN (");
                let mut separated = query_builder.separated(", ");
                for p in projects {
                    separated.push_bind(p);
                }
                separated.push_unseparated(") ");
            }
        }
    }

    if use_fast_path {
        if let Some(ts) = filter.from_ts {
            query_builder.push(" AND date >= date(");
            query_builder.push_bind(ts);
            query_builder.push(", 'unixepoch')");
        }
        if let Some(ts) = filter.to_ts {
            query_builder.push(" AND date <= date(");
            query_builder.push_bind(ts);
            query_builder.push(", 'unixepoch')");
        }
    } else {
        if let Some(ts) = filter.from_ts {
            query_builder.push(" AND created_at >= ");
            query_builder.push_bind(ts);
        }
        if let Some(ts) = filter.to_ts {
            query_builder.push(" AND created_at <= ");
            query_builder.push_bind(ts);
        }
    }

    let query = query_builder.build_query_as::<SummaryStat>();
    let stats = query.fetch_one(&state.db).await.unwrap_or(SummaryStat {
        total_count: 0,
        avg_duration: 0.0,
        success_rate: 0.0,
    });

    if let Ok(val) = serde_json::to_value(&stats) {
        let _ = state.cache.insert(key, val);
    }

    Json(stats)
}


async fn list_pipelines(
    State(state): State<AppState>,
    Query(filter): Query<PipelineFilter>,
) -> Json<Vec<PipelineResponse>> {
    // timestamp now not needed here

    let pipelines = {
        let mut query_builder = sqlx::QueryBuilder::new("SELECT * FROM pipelines WHERE 1=1");

        if let Some(p) = &filter.project_name {
            if p != "All" && !p.is_empty() {
                if p.contains(',') {
                    let projects: Vec<&str> = p.split(',').map(|s| s.trim()).collect();
                    if !projects.is_empty() {
                        query_builder.push(" AND project_full_path IN (");
                        let mut separated = query_builder.separated(", ");
                        for proj in projects {
                            separated.push_bind(proj);
                        }
                        separated.push_unseparated(") ");
                    }
                } else {
                    query_builder.push(" AND project_full_path = ");
                    query_builder.push_bind(p);
                }
            }
        }
        if let Some(r) = &filter.ref_name {
            if r != "All" && !r.is_empty() {
                if r.contains(',') {
                    let refs: Vec<&str> = r.split(',').map(|s| s.trim()).collect();
                    if !refs.is_empty() {
                        query_builder.push(" AND ref_name IN (");
                        let mut separated = query_builder.separated(", ");
                        for rv in refs {
                            separated.push_bind(rv);
                        }
                        separated.push_unseparated(") ");
                    }
                } else {
                    query_builder.push(" AND ref_name = ");
                    query_builder.push_bind(r);
                }
            }
        }
        if let Some(ex) = &filter.exclude_projects {
            if !ex.is_empty() {
                let projects: Vec<&str> = ex.split(',').collect();
                if !projects.is_empty() {
                    query_builder.push(" AND project_full_path NOT IN (");
                    let mut separated = query_builder.separated(", ");
                    for p in projects {
                        separated.push_bind(p);
                    }
                    separated.push_unseparated(") ");
                }
            }
        }

        if let Some(s) = &filter.status {
            query_builder.push(" AND status = ");
            query_builder.push_bind(s);
        }
        
        let is_running_query = filter.status.as_deref() == Some("running");
        if !is_running_query {
            if let Some(ts) = filter.from_ts {
                query_builder.push(" AND created_at >= ");
                query_builder.push_bind(ts);
            }
            if let Some(ts) = filter.to_ts {
                query_builder.push(" AND created_at <= ");
                query_builder.push_bind(ts);
            }
        }

        query_builder.push(" ORDER BY created_at DESC LIMIT 100");

        let query = query_builder.build_query_as::<Pipeline>();
        match query.fetch_all(&state.db).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("list_pipelines query failed: {}", e);
                Vec::new()
            }
        }
    };

    let response: Vec<PipelineResponse> = pipelines.into_iter().map(|p| {
        let created = chrono::Utc
            .timestamp_opt(p.created_at, 0)
            .single()
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let finished = p.finished_at.and_then(|ts| chrono::Utc.timestamp_opt(ts, 0).single().map(|dt| dt.to_rfc3339()));

        PipelineResponse {
            id: p.id,
            project_id: p.project_id,
            project_name: p.project_name,
            project_full_path: p.project_full_path,
            ref_name: p.ref_name,
            sha: p.sha,
            user_name: p.user_name,
            status: p.status.clone(),
            created_at: created,
            finished_at: finished,
            duration: p.duration,
            web_url: p.web_url,
        }
    }).collect();

    Json(response)
}



async fn get_stats_trend(
    State(state): State<AppState>,
    Query(filter): Query<PipelineFilter>,
) -> Json<Vec<DailyStat>> {
    let now = chrono::Utc::now().timestamp();
    let end_ts = filter.to_ts.unwrap_or(now);
    let mut start_ts = filter.from_ts.unwrap_or(now - 30 * 86400);

    // If time range is less than 1 day, default to showing 1 week
    if end_ts - start_ts < 86400 {
        start_ts = end_ts - 7 * 86400;
    }

    // If ref filter is present, we must use pipelines table (slow path)
    // Otherwise use daily_stats (fast path)
    let use_fast_path = filter.ref_name.as_deref().unwrap_or("All") == "All";

    let mut query_builder = if use_fast_path {
        let mut qb = sqlx::QueryBuilder::new(
            r#"
            SELECT 
                date,
                status,
                SUM(count) as count
            FROM daily_stats
            WHERE date >= date(
            "#
        );
        qb.push_bind(start_ts);
        qb.push(", 'unixepoch') AND date <= date(");
        qb.push_bind(end_ts);
        qb.push(", 'unixepoch')");
        qb
    } else {
        let mut qb = sqlx::QueryBuilder::new(
            r#"
            SELECT 
                date(created_at, 'unixepoch') as date,
                status,
                COUNT(*) as count
            FROM pipelines
            WHERE created_at >= 
            "#
        );
        qb.push_bind(start_ts);
        qb.push(" AND created_at <= ");
        qb.push_bind(end_ts);
        qb
    };

    if let Some(p) = &filter.project_name {
        if p != "All" && !p.is_empty() {
            if p.contains(',') {
                let projects: Vec<&str> = p.split(',').map(|s| s.trim()).collect();
                if !projects.is_empty() {
                    query_builder.push(" AND project_full_path IN (");
                    let mut separated = query_builder.separated(", ");
                    for proj in projects {
                        separated.push_bind(proj);
                    }
                    separated.push_unseparated(") ");
                }
            } else {
                query_builder.push(" AND project_full_path = ");
                query_builder.push_bind(p);
            }
        }
    }
    
    if !use_fast_path {
        if let Some(r) = &filter.ref_name {
            if r != "All" && !r.is_empty() {
                if r.contains(',') {
                    let refs: Vec<&str> = r.split(',').map(|s| s.trim()).collect();
                    if !refs.is_empty() {
                        query_builder.push(" AND ref_name IN (");
                        let mut separated = query_builder.separated(", ");
                        for rv in refs {
                            separated.push_bind(rv);
                        }
                        separated.push_unseparated(") ");
                    }
                } else {
                    query_builder.push(" AND ref_name = ");
                    query_builder.push_bind(r);
                }
            }
        }
    }

    if let Some(ex) = &filter.exclude_projects {
        if !ex.is_empty() {
            let projects: Vec<&str> = ex.split(',').collect();
            if !projects.is_empty() {
                query_builder.push(" AND project_full_path NOT IN (");
                let mut separated = query_builder.separated(", ");
                for p in projects {
                    separated.push_bind(p);
                }
                separated.push_unseparated(") ");
            }
        }
    }

    if use_fast_path {
        query_builder.push(" GROUP BY date, status ORDER BY date DESC");
    } else {
        query_builder.push(" GROUP BY 1, 2 ORDER BY 1 DESC");
    }

    let key = format!("trend:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
        if use_fast_path { "fast" } else { "slow" },
        filter.project_name.as_deref().unwrap_or("All"),
        filter.ref_name.as_deref().unwrap_or("All"),
        filter.exclude_projects.as_deref().unwrap_or(""),
        start_ts,
        end_ts,
    );

    if let Some(cached) = state.cache.get(&key) {
        if let Ok(v) = serde_json::from_value::<Vec<DailyStat>>(cached.clone()) {
            return Json(v);
        }
    }

    let query = query_builder.build_query_as::<DailyStat>();
    let stats = query.fetch_all(&state.db).await.unwrap_or_default();

    if let Ok(val) = serde_json::to_value(&stats) {
        let _ = state.cache.insert(key, val);
    }

    Json(stats)
}

async fn list_projects(State(state): State<AppState>) -> Json<Vec<String>> {
    let projects = sqlx::query_scalar("SELECT DISTINCT project_full_path FROM pipelines ORDER BY project_full_path")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let mut names = projects;
    names.sort();
    Json(names)
}

async fn list_refs(State(state): State<AppState>) -> Json<Vec<String>> {
    let refs: Vec<String> = sqlx::query_scalar("SELECT DISTINCT ref_name FROM pipelines ORDER BY ref_name")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    Json(refs)
}