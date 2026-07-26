use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Blocked,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Blocked => "blocked",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub level: RiskLevel,
    pub reasons: Vec<String>,
    #[serde(rename = "matchedRules")]
    pub matched_rules: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyDecision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyOutcome {
    pub decision: PolicyDecision,
    #[serde(rename = "matchedRuleIds")]
    pub matched_rule_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub path: String,
    pub name: String,
    #[serde(rename = "gitRemote", skip_serializing_if = "Option::is_none")]
    pub git_remote: Option<String>,
    #[serde(rename = "gitBranch", skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionInfo {
    pub kind: String,
    pub command: String,
    pub argv: Vec<String>,
    #[serde(rename = "workingDirectory")]
    pub working_directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "expiresAt")]
    pub expires_at: DateTime<Utc>,
    pub agent: AgentInfo,
    pub project: ProjectInfo,
    pub action: ActionInfo,
    pub risk: RiskAssessment,
    pub policy: PolicyOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowOnce,
    DenyOnce,
    AllowSimilar,
    BlockSimilar,
    Expired,
    AutoAllowed,
    AutoBlocked,
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Decision::AllowOnce => "allow_once",
            Decision::DenyOnce => "deny_once",
            Decision::AllowSimilar => "allow_similar",
            Decision::BlockSimilar => "block_similar",
            Decision::Expired => "expired",
            Decision::AutoAllowed => "auto_allowed",
            Decision::AutoBlocked => "auto_blocked",
        };
        write!(f, "{s}")
    }
}

impl Decision {
    pub fn is_allowed(&self) -> bool {
        matches!(
            self,
            Decision::AllowOnce | Decision::AllowSimilar | Decision::AutoAllowed
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalDecision {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub decision: Decision,
    #[serde(rename = "decidedAt")]
    pub decided_at: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub timestamp: DateTime<Utc>,
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "projectPath")]
    pub project_path: String,
    pub command: String,
    #[serde(rename = "riskLevel")]
    pub risk_level: RiskLevel,
    pub decision: Decision,
    pub reason: String,
    #[serde(rename = "durationMs")]
    pub duration_ms: i64,
}
