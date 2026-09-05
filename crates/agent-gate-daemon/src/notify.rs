//! Waking a phone (and through it, a wrist) when an approval is waiting.
//!
//! The daemon's own `/events` stream only reaches a client that is currently
//! connected, which on iOS means an app that is currently on screen - exactly
//! when you do not need to be told. Delivering to a device you have walked away
//! from needs a push service.
//!
//! This is the free stand-in for that: the daemon posts to an [ntfy] topic, a
//! phone app already on the App Store receives it, and the Watch mirrors it.
//! It is deliberately shaped like the APNs path in `docs/watch-plan.md`, so
//! swapping in a real push provider later replaces this module and nothing
//! else: a *wake-up* crosses the relay, and the *decision* does not.
//!
//! Off unless a config file exists, in the same spirit as `--lan`.
//!
//! # What crosses the relay
//!
//! By default, as little as possible: a risk tier, an agent name, a project
//! basename, and a button that opens the approval page. The command text is
//! opt-in, and the page needs no secret in the notification because a paired
//! device already holds its token.
//!
//! Enabling one-tap `decide` actions is a real tradeoff and is opt-in for that
//! reason. Those buttons carry a single-use capability, so the worst a relay
//! operator can do with one is answer the single request it was minted for -
//! not approve anything else, and not for long. Self-host ntfy, or stay on the
//! default `view` actions, if even that is too much.
//!
//! [ntfy]: https://ntfy.sh

use crate::server::AppState;
use agent_gate_policy::{ApprovalRequest, Decision, RiskLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Which buttons the notification carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Actions {
    /// A single button that opens the approval page. Nothing secret is put on
    /// the wire: a paired device already holds its own token.
    #[default]
    View,
    /// Allow and Deny buttons that decide without opening anything, each
    /// carrying a single-use capability for this one request.
    Decide,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyConfig {
    /// ntfy server. Point this at your own instance to keep the wake-up off a
    /// public relay.
    #[serde(default = "default_server")]
    pub server: String,
    /// The topic to publish to. Treat it as a secret: anyone who knows it can
    /// read your notifications.
    pub topic: String,
    /// How the phone reaches this daemon - a LAN address or a Tailscale name.
    /// Decisions and the approval page both go here directly, never through
    /// the relay.
    pub callback_base: String,
    #[serde(default)]
    pub actions: Actions,
    /// Whether the command text may leave the machine. Off by default.
    #[serde(default)]
    pub include_command: bool,
    /// Don't buzz below this tier.
    #[serde(default = "default_min_risk")]
    pub min_risk: RiskLevel,
}

fn default_server() -> String {
    "https://ntfy.sh".to_string()
}
fn default_min_risk() -> RiskLevel {
    RiskLevel::Medium
}

impl NotifyConfig {
    /// Loads the config, or `None` when the file is absent - which is how
    /// notifications stay off until asked for.
    pub fn load(path: &Path) -> anyhow::Result<Option<NotifyConfig>> {
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(path)?;
        if contents.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(serde_yaml::from_str(&contents)?))
    }

    pub fn topic_url(&self) -> String {
        format!("{}/{}", self.server.trim_end_matches('/'), self.topic)
    }
}

/// A single-use permission to answer one request one way.
///
/// Minted per notification and burned on use, so a capability that leaks
/// through a relay cannot be replayed and cannot reach any other request.
#[derive(Debug, Clone)]
pub struct Grant {
    pub request_id: String,
    pub decision: Decision,
}

#[derive(Default)]
pub struct GrantStore {
    grants: Mutex<HashMap<String, Grant>>,
}

impl GrantStore {
    pub fn mint(&self, request_id: &str, decision: Decision) -> String {
        let token = agent_gate_policy::new_id("act");
        self.grants.lock().expect("grant lock").insert(
            token.clone(),
            Grant {
                request_id: request_id.to_string(),
                decision,
            },
        );
        token
    }

    /// Redeems a token, removing it. A second attempt finds nothing.
    pub fn redeem(&self, token: &str) -> Option<Grant> {
        self.grants.lock().expect("grant lock").remove(token)
    }

    /// Drops every grant for a request once it is answered or reaped, so the
    /// store cannot grow without bound and a stale button cannot fire later.
    pub fn revoke_for(&self, request_id: &str) {
        self.grants
            .lock()
            .expect("grant lock")
            .retain(|_, g| g.request_id != request_id);
    }

    pub fn len(&self) -> usize {
        self.grants.lock().expect("grant lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The ntfy publish body. Only the fields we actually set.
#[derive(Debug, Serialize)]
pub struct Publish {
    pub topic: String,
    pub title: String,
    pub message: String,
    pub priority: u8,
    pub tags: Vec<String>,
    pub actions: Vec<Action>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub action: &'static str,
    pub label: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub clear: bool,
}

/// Builds the notification for a request. Split out from sending so the shape
/// can be asserted in tests without a network.
pub fn build_publish(
    config: &NotifyConfig,
    request: &ApprovalRequest,
    grants: &GrantStore,
) -> Publish {
    let base = config.callback_base.trim_end_matches('/');
    let mut actions = Vec::new();

    match config.actions {
        Actions::View => {
            // The page is already paired, so the URL needs no secret.
            actions.push(Action {
                action: "view",
                label: "Open".to_string(),
                url: format!("{base}/"),
                method: None,
                clear: false,
            });
        }
        Actions::Decide => {
            for (label, decision) in [
                ("Allow", Decision::AllowOnce),
                ("Deny", Decision::DenyOnce),
            ] {
                let token = grants.mint(&request.id, decision);
                actions.push(Action {
                    action: "http",
                    label: label.to_string(),
                    url: format!("{base}/act/{token}"),
                    method: Some("POST"),
                    clear: true,
                });
            }
            // iOS support for http actions is not something to bet on, so a
            // way to open the page is always present as well.
            actions.push(Action {
                action: "view",
                label: "Open".to_string(),
                url: format!("{base}/"),
                method: None,
                clear: false,
            });
        }
    }

    let project = &request.project.name;
    let mut message = format!("{} in {project}", request.agent.name);
    if let Some(reason) = request.risk.reasons.first() {
        message.push_str(&format!("\n{reason}"));
    }
    if config.include_command {
        message.push_str(&format!("\n{}", request.action.command));
    }

    Publish {
        topic: config.topic.clone(),
        title: format!("{} approval waiting", tier_word(request.risk.level)),
        message,
        // High risk is worth breaking through a focus mode; medium is not.
        priority: if request.risk.level >= RiskLevel::High { 5 } else { 4 },
        tags: vec![tier_tag(request.risk.level).to_string()],
        actions,
    }
}

fn tier_word(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "Low-risk",
        RiskLevel::Medium => "Medium-risk",
        RiskLevel::High => "High-risk",
        RiskLevel::Blocked => "Blocked",
    }
}

fn tier_tag(level: RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "white_check_mark",
        RiskLevel::Medium => "warning",
        RiskLevel::High => "rotating_light",
        RiskLevel::Blocked => "no_entry",
    }
}

/// Sends the notification, in the background.
///
/// Nothing here may affect the approval itself: a relay being down, slow, or
/// hostile must never delay or change a decision, so this is spawned and its
/// errors are logged rather than returned.
pub fn spawn_for(state: &Arc<AppState>, request: &ApprovalRequest) {
    let Some(config) = state.notify.clone() else {
        return;
    };
    if request.risk.level < config.min_risk {
        return;
    }
    let body = build_publish(&config, request, &state.grants);
    let server = config.server.trim_end_matches('/').to_string();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        match client.post(&server).json(&body).send().await {
            Ok(response) if !response.status().is_success() => {
                eprintln!("notification rejected by {server}: HTTP {}", response.status());
            }
            Err(err) => eprintln!("notification to {server} failed: {err}"),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_gate_policy::*;
    use chrono::Utc;

    fn config(actions: Actions) -> NotifyConfig {
        NotifyConfig {
            server: "https://ntfy.sh".into(),
            topic: "secret-topic".into(),
            callback_base: "http://mac.local:8787".into(),
            actions,
            include_command: false,
            min_risk: RiskLevel::Medium,
        }
    }

    fn request(command: &str, level: RiskLevel) -> ApprovalRequest {
        ApprovalRequest {
            id: "req_1".into(),
            created_at: Utc::now(),
            expires_at: Utc::now(),
            agent: AgentInfo { id: "claude-code".into(), name: "Claude Code".into(), session_id: None },
            project: ProjectInfo { path: "/p".into(), name: "proj".into(), git_remote: None, git_branch: None },
            action: ActionInfo {
                kind: "shell_command".into(),
                command: command.into(),
                argv: vec![],
                working_directory: "/p".into(),
            },
            risk: RiskAssessment { level, reasons: vec!["Installs a package".into()], matched_rules: vec![] },
            policy: PolicyOutcome { decision: PolicyDecision::Ask, matched_rule_ids: vec![] },
            similar_scope: "commands starting with `npm install`".into(),
        }
    }

    /// The default has to be safe without being configured, because the whole
    /// point is that a relay sees as little as possible.
    #[test]
    fn the_command_never_leaves_by_default() {
        let body = build_publish(&config(Actions::View), &request("npm install evil", RiskLevel::Medium), &GrantStore::default());
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("npm install evil"), "the command was put on the wire");
        assert!(json.contains("Claude Code") && json.contains("proj"));
    }

    #[test]
    fn including_the_command_is_opt_in() {
        let mut c = config(Actions::View);
        c.include_command = true;
        let body = build_publish(&c, &request("npm install left-pad", RiskLevel::Medium), &GrantStore::default());
        assert!(body.message.contains("npm install left-pad"));
    }

    /// A `view` notification must carry no capability at all - the paired page
    /// already holds its own token.
    #[test]
    fn view_actions_carry_no_secret() {
        let body = build_publish(&config(Actions::View), &request("npm install x", RiskLevel::Medium), &GrantStore::default());
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(body.actions.len(), 1);
        assert!(!json.contains("act_"), "a grant leaked into a view notification");
    }

    /// One-tap buttons are a capability, so they must be per-request and
    /// single-use rather than the pairing token in disguise.
    #[test]
    fn decide_actions_mint_one_grant_each_and_still_offer_the_page() {
        let grants = GrantStore::default();
        let body = build_publish(&config(Actions::Decide), &request("npm install x", RiskLevel::Medium), &grants);
        assert_eq!(grants.len(), 2, "one grant per decision button");
        let labels: Vec<&str> = body.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, vec!["Allow", "Deny", "Open"]);
    }

    #[test]
    fn a_grant_is_single_use() {
        let grants = GrantStore::default();
        let token = grants.mint("req_1", Decision::AllowOnce);
        assert!(grants.redeem(&token).is_some());
        assert!(grants.redeem(&token).is_none(), "a grant was replayable");
    }

    #[test]
    fn answering_a_request_revokes_its_other_buttons() {
        let grants = GrantStore::default();
        let allow = grants.mint("req_1", Decision::AllowOnce);
        let deny = grants.mint("req_1", Decision::DenyOnce);
        let other = grants.mint("req_2", Decision::AllowOnce);
        grants.revoke_for("req_1");
        assert!(grants.redeem(&allow).is_none());
        assert!(grants.redeem(&deny).is_none(), "a stale Deny could still fire");
        assert!(grants.redeem(&other).is_some(), "an unrelated request was affected");
    }

    #[test]
    fn high_risk_gets_a_higher_priority_than_medium() {
        let g = GrantStore::default();
        let medium = build_publish(&config(Actions::View), &request("npm install x", RiskLevel::Medium), &g);
        let high = build_publish(&config(Actions::View), &request("git push --force", RiskLevel::High), &g);
        assert!(high.priority > medium.priority);
    }
}
