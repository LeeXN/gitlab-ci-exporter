use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub id: u64,
    pub name: String,
    pub path_with_namespace: String,
    pub last_activity_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectPipelineInfo {
    pub id: u64,
    pub name: String,
    pub full_path: String,
    pub web_url: Option<String>,
    pub pipelines: Vec<PipelineInfo>,
}

#[derive(Deserialize)]
pub struct ProjectNode {
    #[serde(deserialize_with = "parse_gid")]
    pub id: u64,
    pub name: String,
    #[serde(rename = "fullPath")]
    pub full_path: String,
    #[serde(rename = "webUrl")]
    pub web_url: Option<String>,
    pub pipelines: Option<PipelineConnection>,
}

#[derive(Deserialize)]
pub struct ProjectConnection {
    #[serde(rename = "pageInfo")]
    pub page_info: Option<PageInfo>,
    pub nodes: Option<Vec<ProjectNode>>,
}

#[derive(Deserialize)]
pub struct PageInfo {
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
}

#[derive(Deserialize)]
pub struct PipelineConnection {
    pub nodes: Option<Vec<PipelineInfo>>,
}

/// Parse GraphQL ID（gid://.../12345）to extract numeric ID
pub fn parse_gid<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s: String = Deserialize::deserialize(deserializer)?;
    s.rsplit('/')
        .next()
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PipelineInfo {
    #[serde(deserialize_with = "parse_gid")]
    pub id: u64,
    pub sha: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "finishedAt")]
    pub finished_at: Option<String>,
    pub duration: Option<u64>,
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub web_url: Option<String>,
    pub user: UserInfo,
}

impl PipelineInfo {
    pub fn to_db_pipeline(&self, project_id: i64, project_name: &str, project_full_path: &str) -> crate::models::Pipeline {
        let created_ts = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);
        let finished_ts = self.finished_at.as_ref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.timestamp())
        });
        // If duration missing but finished timestamp present, compute duration
        let duration = match (self.duration, finished_ts) {
            (Some(d), _) => Some(d as i64),
            (None, Some(f_ts)) => {
                let dur = f_ts - created_ts;
                if dur > 0 { Some(dur as i64) } else { None }
            }
            _ => None,
        };

        crate::models::Pipeline {
            id: self.id as i64,
            project_id,
            project_name: project_name.to_string(),
            project_full_path: project_full_path.to_string(),
            ref_name: self.ref_name.clone(),
            sha: self.sha.clone(),
            user_name: self.user.name.clone(),
            status: self.status.to_ascii_lowercase(),
            created_at: created_ts,
            finished_at: finished_ts,
            duration,
            web_url: self.web_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitlabPipeline {
    pub id: u64,
    pub r#ref: String,
    pub sha: String,
    pub status: String,
    #[serde(rename = "created_at")]
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "updated_at")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub web_url: Option<String>,
    pub duration: Option<u64>,
}

impl GitlabPipeline {
    pub fn to_db_pipeline(&self, project_id: i64, project_name: &str, project_full_path: &str) -> crate::models::Pipeline {
        // compute duration if missing but finished_at is present
        let created_ts = self.created_at.timestamp();
        let finished_ts = self.finished_at.map(|d| d.timestamp());
        let duration = match (self.duration, finished_ts) {
            (Some(d), _) => Some(d as i64),
            (None, Some(f_ts)) => {
                let dur = f_ts - created_ts;
                if dur > 0 { Some(dur as i64) } else { None }
            }
            _ => None,
        };

        crate::models::Pipeline {
            id: self.id as i64,
            project_id,
            project_name: project_name.to_string(),
            project_full_path: project_full_path.to_string(),
            ref_name: self.r#ref.clone(),
            sha: self.sha.clone(),
             // GitLab REST API /api/v4/projects/:ID/pipelines does not provide user info in pipeline object
            user_name: "".to_string(),
            status: self.status.to_ascii_lowercase(),
            created_at: created_ts,
            finished_at: finished_ts,
            duration,
            web_url: self.web_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserInfo {
    pub name: String,
}
