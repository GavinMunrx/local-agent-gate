use crate::types::{PolicyDecision, PolicyOutcome, RiskLevel};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub defaults: RiskDefaults,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

fn default_version() -> u32 {
    1
}

impl Default for PolicyConfig {
    fn default() -> Self {
        PolicyConfig {
            version: 1,
            defaults: RiskDefaults::default(),
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RiskDefaults {
    #[serde(default = "allow_decision", rename = "lowRisk")]
    pub low_risk: PolicyDecision,
    #[serde(default = "ask_decision", rename = "mediumRisk")]
    pub medium_risk: PolicyDecision,
    #[serde(default = "ask_decision", rename = "highRisk")]
    pub high_risk: PolicyDecision,
}

fn allow_decision() -> PolicyDecision {
    PolicyDecision::Allow
}
fn ask_decision() -> PolicyDecision {
    PolicyDecision::Ask
}

impl Default for RiskDefaults {
    fn default() -> Self {
        RiskDefaults {
            low_risk: PolicyDecision::Allow,
            medium_risk: PolicyDecision::Ask,
            high_risk: PolicyDecision::Ask,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    #[serde(rename = "match")]
    pub matcher: RuleMatch,
    pub decision: PolicyDecision,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RuleMatch {
    #[serde(rename = "commandContains")]
    pub command_contains: Option<String>,
    #[serde(rename = "commandStartsWith")]
    pub command_starts_with: Option<String>,
    #[serde(rename = "commandRegex")]
    pub command_regex: Option<String>,
    pub branch: Option<String>,
}

impl RuleMatch {
    fn matches(&self, command: &str, branch: Option<&str>) -> bool {
        if let Some(contains) = &self.command_contains {
            if !command.contains(contains.as_str()) {
                return false;
            }
        }
        if let Some(prefix) = &self.command_starts_with {
            if !command.starts_with(prefix.as_str()) {
                return false;
            }
        }
        if let Some(pattern) = &self.command_regex {
            match regex::Regex::new(pattern) {
                Ok(re) => {
                    if !re.is_match(command) {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
        if let Some(expected_branch) = &self.branch {
            if branch != Some(expected_branch.as_str()) {
                return false;
            }
        }
        true
    }
}

impl PolicyConfig {
    pub fn load_from_dir(dir: &Path) -> anyhow::Result<PolicyConfig> {
        let path = dir.join(".agent-gate.yml");
        if !path.exists() {
            return Ok(PolicyConfig::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        let config: PolicyConfig = serde_yaml::from_str(&contents)?;
        Ok(config)
    }

    /// Evaluates policy rules and risk defaults for a command.
    ///
    /// `risk_level` of `Blocked` always denies, regardless of user rules,
    /// matching the "built-in catastrophic deny" step of the precedence order.
    pub fn evaluate(&self, command: &str, branch: Option<&str>, risk_level: RiskLevel) -> PolicyOutcome {
        if risk_level == RiskLevel::Blocked {
            return PolicyOutcome {
                decision: PolicyDecision::Deny,
                matched_rule_ids: vec!["builtin-catastrophic-deny".to_string()],
            };
        }

        let deny_matches: Vec<&PolicyRule> = self
            .rules
            .iter()
            .filter(|r| r.decision == PolicyDecision::Deny && r.matcher.matches(command, branch))
            .collect();
        if !deny_matches.is_empty() {
            return PolicyOutcome {
                decision: PolicyDecision::Deny,
                matched_rule_ids: deny_matches.iter().map(|r| r.id.clone()).collect(),
            };
        }

        let allow_matches: Vec<&PolicyRule> = self
            .rules
            .iter()
            .filter(|r| r.decision == PolicyDecision::Allow && r.matcher.matches(command, branch))
            .collect();
        if !allow_matches.is_empty() {
            return PolicyOutcome {
                decision: PolicyDecision::Allow,
                matched_rule_ids: allow_matches.iter().map(|r| r.id.clone()).collect(),
            };
        }

        let ask_matches: Vec<&PolicyRule> = self
            .rules
            .iter()
            .filter(|r| r.decision == PolicyDecision::Ask && r.matcher.matches(command, branch))
            .collect();
        if !ask_matches.is_empty() {
            return PolicyOutcome {
                decision: PolicyDecision::Ask,
                matched_rule_ids: ask_matches.iter().map(|r| r.id.clone()).collect(),
            };
        }

        let default_decision = match risk_level {
            RiskLevel::Low => self.defaults.low_risk,
            RiskLevel::Medium => self.defaults.medium_risk,
            RiskLevel::High => self.defaults.high_risk,
            RiskLevel::Blocked => PolicyDecision::Deny,
        };

        PolicyOutcome {
            decision: default_decision,
            matched_rule_ids: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_risk_always_denies_even_with_allow_rule() {
        let mut config = PolicyConfig::default();
        config.rules.push(PolicyRule {
            id: "allow-everything".to_string(),
            matcher: RuleMatch {
                command_contains: Some("rm".to_string()),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        });
        let outcome = config.evaluate("rm -rf ~", None, RiskLevel::Blocked);
        assert_eq!(outcome.decision, PolicyDecision::Deny);
    }

    #[test]
    fn deny_wins_over_allow_when_rules_conflict() {
        let mut config = PolicyConfig::default();
        config.rules.push(PolicyRule {
            id: "allow-publish".to_string(),
            matcher: RuleMatch {
                command_starts_with: Some("npm publish".to_string()),
                ..Default::default()
            },
            decision: PolicyDecision::Allow,
        });
        config.rules.push(PolicyRule {
            id: "deny-publish".to_string(),
            matcher: RuleMatch {
                command_contains: Some("publish".to_string()),
                ..Default::default()
            },
            decision: PolicyDecision::Deny,
        });
        let outcome = config.evaluate("npm publish", None, RiskLevel::High);
        assert_eq!(outcome.decision, PolicyDecision::Deny);
    }

    #[test]
    fn falls_back_to_risk_defaults() {
        let config = PolicyConfig::default();
        assert_eq!(
            config.evaluate("ls", None, RiskLevel::Low).decision,
            PolicyDecision::Allow
        );
        assert_eq!(
            config.evaluate("npm install x", None, RiskLevel::Medium).decision,
            PolicyDecision::Ask
        );
        assert_eq!(
            config.evaluate("npm publish", None, RiskLevel::High).decision,
            PolicyDecision::Ask
        );
    }

    #[test]
    fn branch_scoped_rule_only_matches_target_branch() {
        let mut config = PolicyConfig::default();
        config.rules.push(PolicyRule {
            id: "block-force-push-main".to_string(),
            matcher: RuleMatch {
                command_contains: Some("git push --force".to_string()),
                branch: Some("main".to_string()),
                ..Default::default()
            },
            decision: PolicyDecision::Deny,
        });
        let on_main = config.evaluate("git push --force", Some("main"), RiskLevel::High);
        assert_eq!(on_main.decision, PolicyDecision::Deny);

        let on_feature = config.evaluate("git push --force", Some("feature"), RiskLevel::High);
        assert_eq!(on_feature.decision, PolicyDecision::Ask);
    }
}
