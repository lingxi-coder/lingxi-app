//! Secret scanner with high-confidence gitleaks rules.
//!
//! The bundled binary must not itself contain a complete credential-looking
//! token prefix — Anthropic's pattern is assembled at runtime from fragments.

use regex::Regex;

/// Spec for one detection rule.
///
/// Construction is internal: the canonical rule list is returned by
/// [`builtin_rule_specs`] and compiled into a [`SecretScanner`] via
/// [`SecretScanner::builtin`].
pub struct SecretRuleSpec {
    /// Stable rule identifier (e.g. `aws-access-token`). Embedded in redacted
    /// output as `[REDACTED:<id>]` so reviewers can map back to a rule.
    pub id: &'static str,
    /// Short, human-readable label used in telemetry surfaces.
    pub label: &'static str,
    /// Source regex string. Compiled by [`SecretScanner::builtin`].
    pub source: String,
}

/// One scanner hit. Carries no offset or matched text — safe to forward to
/// telemetry and logs.
#[derive(Debug, Clone)]
pub struct SecretDetection {
    /// Identifier of the rule that fired (matches `SecretRuleSpec::id`).
    pub rule_id: String,
    /// Human-readable label copied from the firing rule.
    pub label: String,
}

/// Compiled scanner over the built-in rule set.
///
/// The scanner is cheap to clone-by-reference (wrap in [`std::sync::Arc`] when
/// shared) and is intended to live for the duration of the process.
pub struct SecretScanner {
    rules: Vec<(String, String, Regex)>, // (id, label, pattern)
}

impl SecretScanner {
    /// Build the standard rule set (subset of gitleaks high-confidence rules).
    ///
    /// Specs whose regex fails to compile are silently dropped so an upstream
    /// pattern bug does not break the entire scanner.
    #[must_use]
    pub fn builtin() -> Self {
        let specs = builtin_rule_specs();
        let rules = specs
            .into_iter()
            .filter_map(|s| {
                Regex::new(&s.source)
                    .ok()
                    .map(|re| (s.id.to_string(), s.label.to_string(), re))
            })
            .collect();
        Self { rules }
    }

    /// Scan `content` and return one [`SecretDetection`] for every firing rule.
    ///
    /// The returned vector is empty when `content` contains no recognized
    /// secrets. Multiple rules can fire on the same input — callers should
    /// treat the result as a multiset.
    #[must_use]
    pub fn scan(&self, content: &str) -> Vec<SecretDetection> {
        let mut hits = Vec::new();
        for (id, label, re) in &self.rules {
            if re.is_match(content) {
                hits.push(SecretDetection {
                    rule_id: id.clone(),
                    label: label.clone(),
                });
            }
        }
        hits
    }

    /// Replace each match with `[REDACTED:<rule-id>]`.
    ///
    /// Rules are applied in order; the output is the input string with every
    /// matching span rewritten. Non-matching content is preserved verbatim.
    #[must_use]
    pub fn redact(&self, content: &str) -> String {
        let mut out = content.to_string();
        for (id, _, re) in &self.rules {
            let replacement = format!("[REDACTED:{id}]");
            out = re.replace_all(&out, replacement.as_str()).to_string();
        }
        out
    }
}

fn builtin_rule_specs() -> Vec<SecretRuleSpec> {
    // Anthropic key prefix built at runtime — avoids bundled-binary scan match.
    let ant_pfx = format!("{}-{}-{}", "sk", "ant", "api");
    let ant_pat = format!(r"\b({ant_pfx}03-[a-zA-Z0-9_\-]{{93}}AA)");

    vec![
        SecretRuleSpec {
            id: "aws-access-token",
            label: "AWS Access Token",
            source: r"\b((?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16})\b".into(),
        },
        SecretRuleSpec {
            id: "gcp-api-key",
            label: "GCP API Key",
            source: r"\b(AIza[\w-]{35})\b".into(),
        },
        SecretRuleSpec {
            id: "anthropic-api-key",
            label: "Anthropic API Key",
            source: ant_pat,
        },
        SecretRuleSpec {
            id: "openai-api-key",
            label: "OpenAI API Key",
            source: r"\bsk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}".into(),
        },
        SecretRuleSpec {
            id: "github-pat",
            label: "GitHub PAT",
            source: r"\bghp_[0-9a-zA-Z]{36}\b".into(),
        },
        SecretRuleSpec {
            id: "github-fine-grained-pat",
            label: "GitHub Fine-Grained PAT",
            source: r"\bgithub_pat_[A-Za-z0-9_]{82}\b".into(),
        },
        SecretRuleSpec {
            id: "gitlab-pat",
            label: "GitLab PAT",
            source: r"\bglpat-[0-9a-zA-Z_\-]{20}\b".into(),
        },
        SecretRuleSpec {
            id: "slack-bot-token",
            label: "Slack Bot Token",
            source: r"\bxox[abprs]-[0-9a-zA-Z\-]{10,72}\b".into(),
        },
        SecretRuleSpec {
            id: "stripe-secret-key",
            label: "Stripe Secret Key",
            source: r"\bsk_(live|test)_[0-9a-zA-Z]{24,}".into(),
        },
        SecretRuleSpec {
            id: "digitalocean-pat",
            label: "DigitalOcean PAT",
            source: r"\bdop_v1_[a-f0-9]{64}\b".into(),
        },
        SecretRuleSpec {
            id: "huggingface-token",
            label: "HuggingFace Token",
            source: r"\bhf_[a-zA-Z]{34}\b".into(),
        },
        // ... 20+ more in production; representative subset here ...
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_github_pat() {
        let s = SecretScanner::builtin();
        let hits = s.scan("token=ghp_1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ");
        assert!(hits.iter().any(|h| h.rule_id == "github-pat"));
    }

    #[test]
    fn redacts_aws_access_token() {
        let s = SecretScanner::builtin();
        let red = s.redact("Hi AKIAIOSFODNN7EXAMPLE bye");
        assert!(!red.contains("AKIA"));
        assert!(red.contains("REDACTED:aws-access-token"));
    }

    #[test]
    fn clean_content_passes() {
        let s = SecretScanner::builtin();
        assert!(s.scan("just normal text here").is_empty());
    }
}
