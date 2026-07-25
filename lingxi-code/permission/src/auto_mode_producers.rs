//! WIZARD-06 — the recon section producers (2.1.220).
//!
//! One function per [`crate::auto_mode_pregather::ReconSection`]. They are kept
//! out of the gather skeleton so each can land independently; the skeleton
//! renders [`crate::auto_mode_pregather::SECTION_FAILED_MARKER`] for any that
//! is not yet ported.
//!
//! Producers behind a closed consent gate are never called at all — see
//! [`crate::auto_mode_pregather::build_recon_block`].

use crate::auto_mode_defaults::{
    default_rule_label, DEFAULT_ALLOW_LABELS, DEFAULT_SOFT_DENY_LABELS,
};
use crate::auto_mode_facts::DEFAULT_LABELS_GUIDANCE;
use crate::auto_mode_sections::{HEADING_DEFAULT_ALLOW_LABELS, HEADING_DEFAULT_SOFT_DENY_LABELS};

/// Render one label list as `- {label}` bullets.
fn label_bullets(labels: &[&str]) -> String {
    labels
        .iter()
        .map(|l| format!("- {}", default_rule_label(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `_ay` — the "Shipped default auto-mode rule labels" section body.
///
/// Lists the labels of the shipped `allow` and `soft_deny` rules so the model
/// can avoid proposing carve-outs the defaults already cover. Labels only: the
/// full rule prose belongs to the classifier prompt, and the section exists to
/// say what is *already covered*, not to restate it.
#[must_use]
pub fn default_labels_section() -> String {
    format!(
        "{DEFAULT_LABELS_GUIDANCE}\n{HEADING_DEFAULT_ALLOW_LABELS}{}\n{HEADING_DEFAULT_SOFT_DENY_LABELS}{}",
        label_bullets(&DEFAULT_ALLOW_LABELS),
        label_bullets(&DEFAULT_SOFT_DENY_LABELS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_labels_section_has_the_oracle_shape() {
        let body = default_labels_section();
        // guidance, then the two sub-headings with their bullet lists.
        assert!(body.starts_with(DEFAULT_LABELS_GUIDANCE));
        assert!(body.contains("\n\n#### Default allow labels\n- Security Discussion\n"));
        assert!(body.contains("\n\n#### Default soft-deny labels\n- Git Destructive\n"));
        // Every shipped label appears exactly once as a bullet.
        for label in DEFAULT_ALLOW_LABELS.iter().chain(DEFAULT_SOFT_DENY_LABELS.iter()) {
            assert_eq!(
                body.matches(&format!("- {label}\n")).count()
                    + usize::from(body.ends_with(&format!("- {label}"))),
                1,
                "label {label:?} must appear exactly once"
            );
        }
    }

    #[test]
    fn shipped_default_slots_match_the_oracle_counts() {
        assert_eq!(crate::auto_mode_defaults::DEFAULT_ENVIRONMENT.len(), 20);
        assert_eq!(DEFAULT_ALLOW_LABELS.len(), 17);
        assert_eq!(DEFAULT_SOFT_DENY_LABELS.len(), 65);
        assert_eq!(crate::auto_mode_defaults::DEFAULT_HARD_DENY_LABELS.len(), 1);
        assert_eq!(
            crate::auto_mode_defaults::DEFAULT_HARD_DENY_LABELS[0],
            "Data Exfiltration"
        );
    }

    #[test]
    fn rule_label_cuts_at_the_first_colon_or_bracket() {
        assert_eq!(default_rule_label("Read-Only Operations: GET requests"), "Read-Only Operations");
        assert_eq!(
            default_rule_label("Git Destructive [named+specifics]: force push"),
            "Git Destructive"
        );
        assert_eq!(default_rule_label("Bare Label"), "Bare Label");
        // Already-reduced labels are unchanged, so applying it twice is safe.
        for label in DEFAULT_ALLOW_LABELS {
            assert_eq!(default_rule_label(label), label);
        }
    }

    #[test]
    fn the_environment_slot_is_verbatim() {
        let env = crate::auto_mode_defaults::DEFAULT_ENVIRONMENT;
        assert_eq!(env[0], "**Organization**: None configured");
        // The `—` escapes in the bundle decoded to real em-dashes.
        assert!(env[4].contains('\u{2014}'));
        // NOTE the classifier template uses STRAIGHT apostrophes, unlike the
        // UI messages elsewhere in this subsystem which use U+2019.
        assert!(env[11].contains("repo's public/private visibility"));
        assert!(!env[11].contains('\u{2019}'));
        // Every entry is a bolded label bullet.
        for e in env {
            assert!(e.starts_with("**"), "{e:?}");
        }
    }
}
