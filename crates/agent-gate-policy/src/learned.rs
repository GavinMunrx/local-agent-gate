//! Rules learned from a human's decisions.
//!
//! "Allow similar" is what turns an approval into something that saves work
//! later - and it is also the one place where a single tap can silently widen
//! the gate. Two properties keep that safe:
//!
//! * A learned rule is scoped to the project it was learned in, so approving
//!   something in a scratch repo cannot loosen a production one.
//! * The generalisation is narrow and mechanical, never fuzzy. A single simple
//!   command generalises to its program and subcommand; anything compound is
//!   pinned to its exact text, because "commands like `a && b`" has no honest
//!   meaning.
//!
//! A learned rule can never override the built-in catastrophic tier: that is
//! enforced in [`crate::policy::PolicyConfig::evaluate`], which denies blocked
//! risk before consulting any rule at all.

use crate::shell;
use crate::types::PolicyDecision;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How a learned rule recognises future commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Match {
    /// Commands starting with this program (and subcommand).
    Prefix { value: String },
    /// Exactly this command text, used when generalising would be dishonest.
    Exact { value: String },
}

impl Match {
    pub fn describe(&self) -> String {
        match self {
            Match::Prefix { value } => format!("commands starting with `{value}`"),
            Match::Exact { value } => format!("exactly `{value}`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedRule {
    pub id: String,
    /// The project this was learned in. Rules never apply outside it.
    pub project_path: String,
    #[serde(rename = "match")]
    pub matcher: Match,
    pub decision: PolicyDecision,
    pub created_at: DateTime<Utc>,
    /// The command that prompted this rule, kept so a human reviewing the list
    /// can see what they actually approved.
    pub learned_from: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LearnedStore {
    #[serde(default)]
    pub rules: Vec<LearnedRule>,
}

impl LearnedStore {
    pub fn load(path: &Path) -> anyhow::Result<LearnedStore> {
        if !path.exists() {
            return Ok(LearnedStore::default());
        }
        let contents = std::fs::read_to_string(path)?;
        if contents.trim().is_empty() {
            return Ok(LearnedStore::default());
        }
        Ok(serde_yaml::from_str(&contents)?)
    }

    /// Writes via a sibling temp file and a rename, so a reader can never see
    /// a half-written file. The daemon re-reads this on every request, which
    /// makes a torn read a live possibility rather than a theoretical one.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file_name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("learned policy path has no file name"))?
            .to_string_lossy()
            .to_string();
        let tmp = path.with_file_name(format!(".{file_name}.tmp"));
        std::fs::write(&tmp, serde_yaml::to_string(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Records a decision as a rule, replacing any existing rule that would
    /// match the same commands in the same project - so deciding the opposite
    /// way later flips the rule rather than stacking a contradiction.
    pub fn learn(
        &mut self,
        project_path: &str,
        command: &str,
        decision: PolicyDecision,
    ) -> LearnedRule {
        let matcher = derive_match(command);
        self.rules
            .retain(|r| !(r.project_path == project_path && r.matcher == matcher));
        let rule = LearnedRule {
            id: crate::new_id("rule"),
            project_path: project_path.to_string(),
            matcher,
            decision,
            created_at: Utc::now(),
            learned_from: command.to_string(),
        };
        self.rules.push(rule.clone());
        rule
    }

    pub fn forget(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != before
    }

    /// The rules that apply to a project, as ordinary policy rules.
    pub fn rules_for(&self, project_path: &str) -> Vec<crate::policy::PolicyRule> {
        self.rules
            .iter()
            .filter(|r| r.project_path == project_path)
            .map(|r| r.as_policy_rule())
            .collect()
    }
}

impl LearnedRule {
    pub fn as_policy_rule(&self) -> crate::policy::PolicyRule {
        let matcher = match &self.matcher {
            Match::Prefix { value } => crate::policy::RuleMatch {
                command_starts_with: Some(value.clone()),
                ..Default::default()
            },
            Match::Exact { value } => crate::policy::RuleMatch {
                command_regex: Some(format!("^{}$", regex::escape(value))),
                ..Default::default()
            },
        };
        crate::policy::PolicyRule {
            id: self.id.clone(),
            matcher,
            decision: self.decision,
        }
    }
}

/// Works out what "similar" means for a command.
///
/// Only a single simple command is generalised, to its program plus a
/// subcommand when there is one. Anything with a pipe, an operator or a
/// substitution keeps its exact text: the parts of a compound command are not
/// interchangeable, so widening one would be guessing.
pub fn derive_match(command: &str) -> Match {
    let trimmed = command.trim();
    let pipelines = shell::parse(trimmed);
    let single = pipelines.len() == 1 && pipelines[0].commands.len() == 1;
    if !single {
        return Match::Exact {
            value: trimmed.to_string(),
        };
    }

    let argv = &pipelines[0].commands[0].argv;
    let Some(program) = argv.first() else {
        return Match::Exact {
            value: trimmed.to_string(),
        };
    };

    // A redirect makes the command write somewhere the prefix does not name,
    // so it is not safely generalisable.
    if pipelines[0].commands[0].writes_file {
        return Match::Exact {
            value: trimmed.to_string(),
        };
    }

    let mut prefix = program.clone();
    if let Some(second) = argv.get(1) {
        if !second.starts_with('-') {
            prefix.push(' ');
            prefix.push_str(second);
        }
    }
    Match::Prefix { value: prefix }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_simple_command_generalises_to_program_and_subcommand() {
        assert_eq!(
            derive_match("npm install left-pad"),
            Match::Prefix {
                value: "npm install".into()
            }
        );
        assert_eq!(
            derive_match("cargo build --release"),
            Match::Prefix {
                value: "cargo build".into()
            }
        );
        // No subcommand to borrow: the program alone.
        assert_eq!(
            derive_match("ls -la"),
            Match::Prefix { value: "ls".into() }
        );
    }

    #[test]
    fn compound_commands_are_never_generalised() {
        // "Commands like `a && b`" has no honest meaning, so pin the text.
        for command in [
            "cd /tmp && npm install",
            "cat f | grep x",
            "echo $(whoami)",
            "npm install; npm test",
        ] {
            assert!(
                matches!(derive_match(command), Match::Exact { .. }),
                "{command} should not have been generalised"
            );
        }
    }

    #[test]
    fn redirection_is_not_generalised() {
        // `echo x > a` and `echo x > b` write to different places.
        assert!(matches!(
            derive_match("echo hi > out.txt"),
            Match::Exact { .. }
        ));
    }

    #[test]
    fn learning_is_scoped_to_one_project() {
        let mut store = LearnedStore::default();
        store.learn("/a", "npm install left-pad", PolicyDecision::Allow);
        assert_eq!(store.rules_for("/a").len(), 1);
        assert_eq!(
            store.rules_for("/b").len(),
            0,
            "a rule learned in one project must not leak into another"
        );
    }

    #[test]
    fn deciding_the_other_way_replaces_rather_than_stacks() {
        let mut store = LearnedStore::default();
        store.learn("/a", "npm install left-pad", PolicyDecision::Allow);
        store.learn("/a", "npm install right-pad", PolicyDecision::Deny);
        let rules = store.rules_for("/a");
        assert_eq!(rules.len(), 1, "same prefix must not stack contradictions");
        assert_eq!(rules[0].decision, PolicyDecision::Deny);
    }

    #[test]
    fn forget_removes_a_rule() {
        let mut store = LearnedStore::default();
        let rule = store.learn("/a", "ls", PolicyDecision::Allow);
        assert!(store.forget(&rule.id));
        assert!(!store.forget(&rule.id), "forgetting twice is not an error");
        assert_eq!(store.rules_for("/a").len(), 0);
    }

    #[test]
    fn round_trips_through_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learned.yml");
        let mut store = LearnedStore::default();
        store.learn("/a", "cargo test", PolicyDecision::Allow);
        store.save(&path).unwrap();
        let loaded = LearnedStore::load(&path).unwrap();
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].learned_from, "cargo test");
    }

    #[test]
    fn an_exact_rule_matches_only_that_command() {
        let mut store = LearnedStore::default();
        store.learn("/a", "cd /tmp && npm install", PolicyDecision::Allow);
        let rule = &store.rules_for("/a")[0];
        assert!(rule.matcher.matches("cd /tmp && npm install", None));
        assert!(!rule.matcher.matches("cd /tmp && npm install evil", None));
    }
}
