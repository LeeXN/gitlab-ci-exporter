use crate::gitlab_ops;
use crate::state::AppState;
use chrono::Utc;
use regex::Regex;
use std::time::Duration as StdDuration;
use tokio::time::sleep;
use tracing::{error, info};
use crate::db;
use chrono::TimeZone;

pub async fn perform_initial_backfill(state: AppState) {
    info!("Starting initial backfill via REST API...");
    
    let branch_filter = if let Some(re) = &state.config.gitlab.branch_filter_regex {
        match Regex::new(re) {
            Ok(r) => Some(r),
            Err(e) => {
                error!("Invalid branch filter regex: {}", e);
                None
            }
        }
    } else {
        None
    };

    info!("Discovering all projects for backfill...");
    let projects = match gitlab_ops::discover_projects(&state.gitlab_client, &state.config.gitlab.monitor_groups, None).await {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to discover projects: {}", e);
            return;
        }
    };
    info!("Discovered {} projects for backfill", projects.len());

    // store monitored projects in state for API listing
    {
        let mut mp = state.monitored_projects.write().unwrap();
        mp.clear();
        for pr in &projects {
            mp.push(pr.clone());
        }
    }

    let backfill_cutoff = chrono::Utc::now().timestamp() - (state.config.poller.backfill_days * 86400);
    let updated_after = Some(chrono::DateTime::from_timestamp(backfill_cutoff, 0).unwrap_or_default());

    // Concurrently fetch pipelines for projects in batches
    let concurrency: usize = 10;
    let mut id_to_project = std::collections::HashMap::new();
    let mut project_ids = Vec::new();
    for project in projects.iter() {
        id_to_project.insert(project.id, project.clone());
        project_ids.push(project.id);
    }

    info!("Fetching pipelines for {} projects concurrently (concurrency={})", project_ids.len(), concurrency);
    match gitlab_ops::fetch_pipelines_concurrent(&state.gitlab_client, project_ids, updated_after, concurrency).await {
        Ok(results) => {
            for (pid, pipelines) in results {
                let project = match id_to_project.get(&pid) {
                    Some(p) => p,
                    None => continue,
                };
                info!("Fetched {} pipelines for project {}", pipelines.len(), project.name);
                for p in pipelines {
                    if let Some(re) = &branch_filter {
                        if !re.is_match(&p.r#ref) { continue; }
                    }
                    let db_p = p.to_db_pipeline(project.id as i64, &project.name, &project.path_with_namespace);
                    insert_pipeline(&state, db_p).await;
                }
            }
        }
        Err(e) => error!("Concurrent fetch_pipelines failed: {}", e),
    }
    
    info!("Initial backfill complete.");
}

pub async fn backfill_usernames(state: AppState) {
    use tokio::task::JoinSet;

    tracing::info!("Starting username backfill for pipelines with missing user_name");

    // loop until no more missing user_name
    loop {
        // fetch a batch of pipeline ids with missing user_name and their project_id
        let rows: Vec<(i64, i64)> = match sqlx::query_as("SELECT id, project_id FROM pipelines WHERE user_name IS NULL OR user_name = '' LIMIT 500")
            .fetch_all(&state.db).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to query pipelines for username backfill: {}", e);
                return;
            }
        };

        if rows.is_empty() {
            tracing::info!("No pipelines found with missing user_name; username backfill complete");
            break;
        }

        // process in chunks with limited concurrency
        let ids: Vec<(i64,i64)> = rows.into_iter().map(|(id, pid)| (id, pid)).collect();
        let concurrency: usize = 10;

        for chunk in ids.chunks(50) {
            let mut set: JoinSet<(i64, Option<String>)> = JoinSet::new();
            for &(pid, proj_id) in chunk {
                let gclient = state.graphql_client.clone();
                set.spawn(async move {
                    let gid = format!("gid://gitlab/Ci::Pipeline/{}", pid);
                    // Try GraphQL first
                    match gclient.fetch_pipeline_user_by_gid(&gid).await {
                        Ok(Some(name)) => (pid, Some(name)),
                        Ok(None) => {
                            // GraphQL returned no user; try REST fallback
                            match gclient.fetch_pipeline_user_via_rest(proj_id, pid).await {
                                Ok(opt) => (pid, opt),
                                Err(e) => {
                                    tracing::error!("REST fetch for pipeline {} failed: {}", pid, e);
                                    (pid, None)
                                }
                            }
                        }
                        Err(_e) => {
                            // GraphQL failed; try REST
                            match gclient.fetch_pipeline_user_via_rest(proj_id, pid).await {
                                Ok(opt) => (pid, opt),
                                Err(e) => {
                                    tracing::error!("Both GraphQL and REST fetch failed for pipeline {}: {}", pid, e);
                                    (pid, None)
                                }
                            }
                        }
                    }
                });

                if set.len() >= concurrency { break; }
            }

            while let Some(res) = set.join_next().await {
                match res {
                    Ok((pid, Some(name))) => {
                        if let Err(e) = sqlx::query("UPDATE pipelines SET user_name = ? WHERE id = ? AND (user_name IS NULL OR user_name = '')")
                            .bind(&name)
                            .bind(pid)
                            .execute(&state.db).await {
                            tracing::error!("Failed to update user_name for pipeline {}: {}", pid, e);
                        } else {
                            tracing::info!("Backfilled pipeline {} -> user={} ", pid, name);
                        }
                    }
                    Ok((_pid, None)) => { /* nothing to update */ }
                    Err(e) => { tracing::error!("Task join error during username backfill: {}", e); }
                }
            }

            // small sleep to avoid hammering the API
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

pub async fn start_monitor_loop(state: AppState) {
    let branch_filter = if let Some(re) = &state.config.gitlab.branch_filter_regex {
        match Regex::new(re) {
            Ok(r) => Some(r),
            Err(e) => {
                error!("Invalid branch filter regex: {}", e);
                None
            }
        }
    } else {
        None
    };

    loop {
        let current_loop_start = Utc::now();
        info!("Starting polling cycle at {}", current_loop_start);
        for group_path in &state.config.gitlab.monitor_groups {
            info!("Polling group: {}", group_path);
            // per requirements: generate poll time, read last poll, write current poll time immediately,
            // then use last poll as `updatedAfter` for GraphQL query to avoid gaps.
            let poll_time = chrono::Utc::now();
            let last_poll_ts = match db::get_last_poll(&state.db).await {
                Ok(opt) => opt.unwrap_or(poll_time.timestamp()),
                Err(e) => {
                    error!("Failed to read last poll watermark: {}", e);
                    poll_time.timestamp()
                }
            };

            let since_time = chrono::Utc.timestamp_opt(last_poll_ts, 0).single().unwrap_or(poll_time);
            info!("Fetching activity since {}", since_time);

            match state.graphql_client.fetch_incremental_activity(group_path, since_time).await {
                Ok(projects) => {
                    // fetch succeeded — update watermark to current poll_time
                    if let Err(e) = db::set_last_poll(&state.db, current_loop_start.timestamp()).await {
                        error!("Failed to update poll watermark after successful fetch: {}", e);
                    }
                    for proj in projects {
                        for pipeline in proj.pipelines {
                            if let Some(re) = &branch_filter {
                                if !re.is_match(&pipeline.ref_name) { continue; }
                            }
                            let db_p = pipeline.to_db_pipeline(proj.id as i64, &proj.name, &proj.full_path);
                            insert_pipeline(&state, db_p).await;
                            info!("Processed pipeline {} for project {}", pipeline.id, proj.name);
                        }
                    }
                },
                Err(e) => {
                    error!("Failed to fetch activity for group {}: {}", group_path, e);
                },
            }
        }

        info!("Polling cycle complete. Next poll in {} seconds.", state.config.poller.interval_seconds);
        
        tokio::select! {
            _ = sleep(StdDuration::from_secs(state.config.poller.interval_seconds)) => {}
            _ = state.refresh_notify.notified() => {
                info!("Received force refresh signal.");
            }
        }
    }
}

async fn insert_pipeline(state: &AppState, p: crate::models::Pipeline) {
    // Use a transaction to upsert pipeline and maintain daily_stats atomically
    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(e) => { error!("Failed to begin transaction: {}", e); return; }
    };

    // Fetch existing pipeline if any
    let existing: Option<(String, Option<i64>, i64)> = match sqlx::query_as(
        "SELECT status, duration, created_at FROM pipelines WHERE id = ?",
    ).bind(p.id).fetch_optional(&mut *tx).await {
        Ok(r) => r,
        Err(e) => { error!("Failed to query existing pipeline {}: {}", p.id, e); let _ = tx.rollback().await; return; }
    };

    // Upsert pipeline row
    match sqlx::query(
        r#"
        INSERT INTO pipelines (id, project_id, project_name, project_full_path, ref_name, user_name, sha, status, created_at, finished_at, web_url, duration)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            status = CASE
                WHEN excluded.finished_at IS NULL AND pipelines.finished_at IS NOT NULL THEN pipelines.status
                ELSE excluded.status
            END,
            finished_at = CASE
                WHEN excluded.finished_at IS NOT NULL THEN excluded.finished_at
                ELSE pipelines.finished_at
            END,
            sha = excluded.sha,
            duration = CASE
                WHEN excluded.duration IS NOT NULL THEN excluded.duration
                ELSE pipelines.duration
            END,
            web_url = COALESCE(excluded.web_url, pipelines.web_url),
            user_name = COALESCE(excluded.user_name, pipelines.user_name)
        "#,
    ).bind(p.id)
    .bind(p.project_id)
    .bind(&p.project_name)
    .bind(&p.project_full_path)
    .bind(&p.ref_name)
    .bind(&p.user_name)
    .bind(&p.sha)
    .bind(&p.status)
    .bind(p.created_at)
    .bind(p.finished_at)
    .bind(&p.web_url)
    .bind(p.duration)
    .execute(&mut *tx).await {
        Err(e) => { error!("Failed to upsert pipeline {}: {}", p.id, e); let _ = tx.rollback().await; return; }
        Ok(_) => {}
    }

    // Prepare date string for aggregation (YYYY-MM-DD)
    let p_date = chrono::Utc.timestamp_opt(p.created_at, 0).single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_string());

    // Helper: track duration presence
    let new_has_dur = p.duration.is_some();
    let new_dur = p.duration.unwrap_or(0);

    if let Some((old_status, old_dur_opt, old_created_at)) = existing {
        let old_has_dur = old_dur_opt.is_some();
        let old_dur = old_dur_opt.unwrap_or(0);
        let old_date = chrono::Utc.timestamp_opt(old_created_at, 0).single()
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| p_date.clone());

        if old_status == p.status {
            // Same status: handle duration presence/changes
            match (old_has_dur, new_has_dur) {
                (true, true) => {
                    // both have durations: adjust total_duration by delta
                    let delta = new_dur - old_dur;
                    if delta != 0 {
                        if let Err(e) = sqlx::query("UPDATE daily_stats SET total_duration = total_duration + ? WHERE date = ? AND project_id = ? AND status = ?")
                            .bind(delta)
                            .bind(&p_date)
                            .bind(p.project_id)
                            .bind(&p.status)
                            .execute(&mut *tx).await {
                            error!("Failed to update daily_stats duration for pipeline {}: {}", p.id, e);
                            let _ = tx.rollback().await; return;
                        }
                    }
                }
                (false, true) => {
                    // previously no duration -> now has: increment count_with_duration and add duration
                    if let Err(e) = sqlx::query("UPDATE daily_stats SET total_duration = total_duration + ?, count_with_duration = count_with_duration + 1 WHERE date = ? AND project_id = ? AND status = ?")
                        .bind(new_dur)
                        .bind(&p_date)
                        .bind(p.project_id)
                        .bind(&p.status)
                        .execute(&mut *tx).await {
                        error!("Failed to update daily_stats adding duration for pipeline {}: {}", p.id, e);
                        let _ = tx.rollback().await; return;
                    }
                }
                (true, false) => {
                    // previously had duration -> now none: subtract old and decrement count_with_duration
                    if let Err(e) = sqlx::query("UPDATE daily_stats SET total_duration = total_duration - ?, count_with_duration = count_with_duration - 1 WHERE date = ? AND project_id = ? AND status = ?")
                        .bind(old_dur)
                        .bind(&p_date)
                        .bind(p.project_id)
                        .bind(&p.status)
                        .execute(&mut *tx).await {
                        error!("Failed to update daily_stats removing duration for pipeline {}: {}", p.id, e);
                        let _ = tx.rollback().await; return;
                    }
                }
                (false, false) => { /* nothing to do */ }
            }
        } else {
            // Status changed: decrement old status row, adjust its duration/count_with_duration
            if let Err(e) = sqlx::query("UPDATE daily_stats SET count = count - 1, total_duration = total_duration - ?, count_with_duration = count_with_duration - ? WHERE date = ? AND project_id = ? AND status = ?")
                .bind(old_dur)
                .bind(if old_has_dur { 1 } else { 0 })
                .bind(&old_date)
                .bind(p.project_id)
                .bind(&old_status)
                .execute(&mut *tx).await {
                error!("Failed to decrement old daily_stats for pipeline {}: {}", p.id, e);
                let _ = tx.rollback().await; return;
            }

            // increment new status (insert or update) with duration info
            if let Err(e) = sqlx::query("INSERT INTO daily_stats(date, project_id, project_name, status, count, total_duration, count_with_duration) VALUES (?, ?, ?, ?, 1, ?, ?) ON CONFLICT(date, project_id, status) DO UPDATE SET count = daily_stats.count + 1, total_duration = daily_stats.total_duration + excluded.total_duration, count_with_duration = daily_stats.count_with_duration + excluded.count_with_duration, project_name = excluded.project_name")
                .bind(&p_date)
                .bind(p.project_id)
                .bind(&p.project_full_path)
                .bind(&p.status)
                .bind(new_dur)
                .bind(if new_has_dur { 1 } else { 0 })
                .execute(&mut *tx).await {
                error!("Failed to increment new daily_stats for pipeline {}: {}", p.id, e);
                let _ = tx.rollback().await; return;
            }
        }
    } else {
        // New pipeline: increment count and total_duration/count_with_duration for its date/status
        if let Err(e) = sqlx::query("INSERT INTO daily_stats(date, project_id, project_name, status, count, total_duration, count_with_duration) VALUES (?, ?, ?, ?, 1, ?, ?) ON CONFLICT(date, project_id, status) DO UPDATE SET count = daily_stats.count + 1, total_duration = daily_stats.total_duration + excluded.total_duration, count_with_duration = daily_stats.count_with_duration + excluded.count_with_duration, project_name = excluded.project_name")
            .bind(&p_date)
            .bind(p.project_id)
            .bind(&p.project_full_path)
            .bind(&p.status)
            .bind(new_dur)
            .bind(if new_has_dur { 1 } else { 0 })
            .execute(&mut *tx).await {
            error!("Failed to insert daily_stats for new pipeline {}: {}", p.id, e);
            let _ = tx.rollback().await; return;
        }
    }

    if let Err(e) = tx.commit().await {
        error!("Failed to commit pipeline insert transaction for {}: {}", p.id, e);
    }
}