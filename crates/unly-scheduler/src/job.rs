use serde::{Deserialize, Serialize};
use unly_core::types::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobType {
    Cron,
    Webhook,
    Adhoc,
    HealthCheck,
}

impl std::fmt::Display for JobType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobType::Cron => write!(f, "cron"),
            JobType::Webhook => write!(f, "webhook"),
            JobType::Adhoc => write!(f, "adhoc"),
            JobType::HealthCheck => write!(f, "health_check"),
        }
    }
}

/// A persistent job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDefinition {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub job_type: JobType,
    pub cron_expression: Option<String>,
    pub payload: serde_json::Value,
    pub enabled: bool,
    pub retry_limit: i64,
}

impl JobDefinition {
    pub fn cron(
        id: impl Into<String>,
        name: impl Into<String>,
        cron_expr: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            job_type: JobType::Cron,
            cron_expression: Some(cron_expr.into()),
            payload,
            enabled: true,
            retry_limit: 3,
        }
    }
}
