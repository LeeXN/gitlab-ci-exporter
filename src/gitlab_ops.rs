use crate::gitlab_types::{GitlabPipeline, ProjectInfo};
use anyhow::Result;
use chrono::{DateTime, Utc};
use gitlab::api::{groups, projects, AsyncQuery, Pagination, paged};
use gitlab::AsyncGitlab;


pub async fn discover_projects(
    client: &AsyncGitlab,
    groups: &[String],
    _min_activity_date: Option<DateTime<Utc>>,
) -> Result<Vec<ProjectInfo>> {
    // User request: Fetch ALL projects (except archived), then filter for pipelines later.
    // We ignore min_activity_date for the API call to ensure we have a complete project list.
    
    let mut all_projects = Vec::new();

    for group in groups {
        // Try to parse group as ID or Name.
        // The endpoint `groups::projects::GroupProjects` takes a group ID or path.
        let mut builder = groups::projects::GroupProjects::builder();
        builder.group(group.as_str());
        builder.include_subgroups(true);
        builder.archived(false);

        let endpoint = builder.build()?;
        
        let projects: Vec<ProjectInfo> = paged(endpoint, Pagination::All)
            .query_async(client)
            .await?;

        for p in projects {
            all_projects.push(p);
        }
    }

    Ok(all_projects)
}

pub async fn fetch_pipelines(
    client: &AsyncGitlab,
    project_id: u64,
    updated_after: Option<DateTime<Utc>>,
) -> Result<Vec<GitlabPipeline>> {
    let mut builder = projects::pipelines::Pipelines::builder();
    builder.project(project_id);
    
    if let Some(after) = updated_after {
        builder.updated_after(after);
    }

    let endpoint = builder.build()?;
    let pipelines: Vec<GitlabPipeline> = paged(endpoint, Pagination::All)
        .query_async(client)
        .await?;
    Ok(pipelines)
}

/// Fetch pipelines for multiple projects concurrently with a concurrency limit.
pub async fn fetch_pipelines_concurrent(
    client: &AsyncGitlab,
    project_ids: Vec<u64>,
    updated_after: Option<DateTime<Utc>>,
    concurrency: usize,
) -> Result<Vec<(u64, Vec<GitlabPipeline>)>> {
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    use std::sync::Arc;
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut join_set: JoinSet<(u64, Result<Vec<GitlabPipeline>, anyhow::Error>)> = JoinSet::new();

    for pid in project_ids {
        let client = client.clone();
        let sem_clone = sem.clone();
        let after = updated_after.clone();
        join_set.spawn(async move {
            // Acquire permit to limit concurrency
            let permit = sem_clone.acquire_owned().await.unwrap();
            let _permit = permit; // keep until the end of this async block

            // Retry logic with exponential backoff
            let mut attempt: u32 = 0;
            let max_retries: u32 = 3;
            loop {
                attempt += 1;
                match fetch_pipelines(&client, pid, after).await {
                    Ok(pipes) => return (pid, Ok(pipes)),
                    Err(e) => {
                        if attempt > max_retries {
                            return (pid, Err(e.into()));
                        }
                        // exponential backoff: 500ms * 2^(attempt-1)
                        let backoff_ms = 500u64.saturating_mul(1u64 << (attempt - 1));
                        tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                        continue;
                    }
                }
            }
        });
    }

    let mut results = Vec::new();
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((pid, Ok(pipes))) => results.push((pid, pipes)),
            Ok((pid, Err(e))) => {
                tracing::error!("fetch_pipelines failed for {}: {}", pid, e);
                results.push((pid, Vec::new()));
            }
            Err(e) => {
                tracing::error!("task join error: {}", e);
            }
        }
    }

    Ok(results)
}
