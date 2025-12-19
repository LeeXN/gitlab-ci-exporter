use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned, Serialize};
use serde_json::json;
use crate::gitlab_types::{ProjectPipelineInfo, ProjectConnection};

#[derive(Clone)]
pub struct GitlabGraphqlClient {
    client: Client,
    base_url: String,
    token: String,
}

#[derive(Deserialize, Serialize)]
struct RawGraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize, Serialize)]
struct GraphQLError {
    message: String,
}

#[derive(Deserialize)]
struct GroupQueryResponse {
    data: Option<GroupData>,
}

#[derive(Deserialize)]
struct GroupData {
    group: Option<GroupNode>,
}

#[derive(Deserialize)]
struct GroupNode {
    projects: Option<ProjectConnection>,
}

impl GitlabGraphqlClient {
    pub fn new(base_url: String, token: String, timeout: u64, skip_invalid_certs: bool) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout)) 
            .danger_accept_invalid_certs(skip_invalid_certs) 
            .build()
            .unwrap_or_default();

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

                pub async fn fetch_pipeline_user_by_gid(&self, gid: &str) -> Result<Option<String>> {
                        // Use the generic `node(id: $id)` entrypoint and inline fragments to support
                        // different GraphQL type names (Pipeline / CiPipeline) across GitLab versions.
                        let query = r#"
                        query($id: ID!) {
                            node(id: $id) {
                                ... on Pipeline {
                                    user { name }
                                }
                                ... on CiPipeline {
                                    user { name }
                                }
                            }
                        }
                        "#;

                        #[derive(Deserialize)]
                        struct PipelineResp {
                                node: Option<PipelineNode>,
                        }

                        #[derive(Deserialize)]
                        struct PipelineNode {
                                user: Option<UserNode>,
                        }

                        #[derive(Deserialize)]
                        struct UserNode {
                                name: Option<String>,
                        }

                        let vars = json!({"id": gid});
                        let resp: PipelineResp = self.post_graphql(query, vars).await?;
                        Ok(resp.node.and_then(|n| n.user.and_then(|u| u.name)))
                }

                    pub async fn fetch_pipeline_user_via_rest(&self, project_id: i64, pipeline_id: i64) -> Result<Option<String>> {
                        let url = format!("{}/api/v4/projects/{}/pipelines/{}", self.base_url, project_id, pipeline_id);
                        let resp = self.client.get(&url)
                            .header("PRIVATE-TOKEN", &self.token)
                            .header("Content-Type", "application/json")
                            .send()
                            .await
                            .context("Failed to send REST request for pipeline")?;

                        if !resp.status().is_success() {
                            let status = resp.status();
                            let text = resp.text().await.unwrap_or_default();
                            bail!("REST HTTP Error {}: {}", status, text);
                        }

                        let v: serde_json::Value = resp.json().await.context("Failed to parse REST JSON")?;
                        let user_name = v.get("user").and_then(|u| u.get("name")).and_then(|n| n.as_str()).map(|s| s.to_string());
                        Ok(user_name)
                    }

    pub async fn fetch_incremental_activity(
        &self, 
        group_full_path: &str, 
        since_time: DateTime<Utc>
    ) -> Result<Vec<ProjectPipelineInfo>> {
        
        let query_time = since_time - Duration::seconds(60);

                let query = r#"
                query($fullPath: ID!, $cursor: String, $updatedAfter: Time!) {
                    group(fullPath: $fullPath) {
                        projects(includeSubgroups: true, first: 50, after: $cursor) {
                            pageInfo {
                                endCursor
                                hasNextPage
                            }
                            nodes {
                                id
                                fullPath
                                name
                                webUrl
                                pipelines(updatedAfter: $updatedAfter, first: 30) {
                                    nodes {
                                        id
                                        sha
                                        status
                                        createdAt
                                        finishedAt
                                        duration
                                        ref
                                        user {
                                            name
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                "#;

        let mut active_projects = Vec::new();
        let mut cursor: Option<String> = None;
        let mut has_next_page = true;

        while has_next_page {
            let variables = json!({
                "fullPath": group_full_path,
                "cursor": cursor,
                "updatedAfter": query_time.to_rfc3339()
            });

            let response: GroupQueryResponse = self.post_graphql(query, variables).await?;

            if let Some(group) = response.data.and_then(|d| d.group) {
                if let Some(projects) = group.projects {
                    if let Some(page_info) = projects.page_info {
                        has_next_page = page_info.has_next_page;
                        cursor = page_info.end_cursor;
                    } else {
                        has_next_page = false;
                    }

                    if let Some(nodes) = projects.nodes {
                        for p in nodes {
                            if let Some(pipe_conn) = p.pipelines {
                                if let Some(mut pipe_nodes) = pipe_conn.nodes {
                                    if !pipe_nodes.is_empty() {
                                        for pipe in &mut pipe_nodes {
                                            let base = p.web_url.as_deref().unwrap_or("");
                                            pipe.web_url = Some(format!("{}/-/pipelines/{}", base, pipe.id));
                                        }

                                        active_projects.push(ProjectPipelineInfo {
                                            id: p.id,
                                            name: p.name,
                                            full_path: p.full_path,
                                            web_url: p.web_url,
                                            pipelines: pipe_nodes,
                                        });
                                    }
                                }
                            }
                        }
                    }
                } else {
                    has_next_page = false;
                }
            } else {
                bail!("Group not found: {}", group_full_path);
            }
        }

        Ok(active_projects)
    }

    async fn post_graphql<T: DeserializeOwned>(&self, query: &str, variables: serde_json::Value) -> Result<T> {
        let payload = json!({
            "query": query,
            "variables": variables
        });

        let response = self.client.post(format!("{}/api/graphql", self.base_url))
            .header("PRIVATE-TOKEN", &self.token) // 注意 header 名称
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send GraphQL request")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            bail!("GraphQL HTTP Error {}: {}", status, text);
        }

        let body: RawGraphQLResponse<serde_json::Value> = response.json().await.context("Failed to parse JSON")?;

        if let Some(ref errors) = body.errors {
            if !errors.is_empty() {
                let msg = errors.iter().map(|e| e.message.as_str()).collect::<Vec<_>>().join(", ");
                bail!("GraphQL API Error: {}", msg);
            }
        }

        // 重新序列化为强类型 struct
        let data = serde_json::from_value(serde_json::to_value(body).unwrap())?;
        Ok(data)
    }
}

// `parse_gid` moved to `crate::gitlab_types::parse_gid`