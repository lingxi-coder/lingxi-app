//! WIZARD-06 — the `--propose` adapters.
//!
//! Two seams sit between [`permission::auto_mode_propose::run_propose`] and the
//! outside world: the recon GATHER (filesystem) and the model QUERY (network).
//! This module supplies both.
//!
//! The query side is deliberately split in two: [`collect_propose_reply`] is a
//! pure function over the event stream, and the live call that produces those
//! events is the caller's business. That way the part with the interesting
//! behaviour — accumulating text and mapping the stop reason — is tested
//! without a network, and the part that needs credentials stays at the edge.

use llm_client::{ContentDelta, LlmError, LlmEvent};
use permission::auto_mode_pregather::{build_recon_block, GatherOptions};
use permission::auto_mode_propose::{ProposeAnswers, ProposeGather, QueryOutcome};

/// Stop reasons that mean "the model ran out of room", across providers.
///
/// Anthropic reports `max_tokens`; the OpenAI family reports `length`.
const TRUNCATING_STOP_REASONS: [&str; 2] = ["max_tokens", "length"];
/// The stop reason that means the model declined.
const REFUSAL_STOP_REASON: &str = "refusal";
/// The only stop reason that yields a usable reply.
const OK_STOP_REASON: &str = "end_turn";

/// Fold a propose reply's event stream into a [`QueryOutcome`].
///
/// Only `end_turn` yields text. Everything else is classified rather than
/// returned as a partial document: a truncated reply is not a shorter
/// proposal, it is an unusable one, and handing its prefix to the parser would
/// turn a clear failure into a confusing parse error.
#[must_use]
pub fn collect_propose_reply(events: Vec<Result<LlmEvent, LlmError>>) -> QueryOutcome {
    let mut text = String::new();
    let mut stop_reason: Option<String> = None;

    for event in events {
        match event {
            Err(_) => return QueryOutcome::Failed("stream error".to_string()),
            Ok(LlmEvent::ContentBlockDelta {
                delta: ContentDelta::TextDelta { text: chunk },
                ..
            }) => text.push_str(&chunk),
            Ok(LlmEvent::MessageDelta { delta, .. }) => {
                if let Some(reason) = delta.stop_reason {
                    stop_reason = Some(reason);
                }
            }
            Ok(_) => {}
        }
    }

    match stop_reason.as_deref() {
        Some(OK_STOP_REASON) => QueryOutcome::Text(text),
        Some(r) if TRUNCATING_STOP_REASONS.contains(&r) => QueryOutcome::Truncated,
        Some(REFUSAL_STOP_REASON) => QueryOutcome::Refused,
        // A stream that ended without ever reporting a terminal stop reason is
        // not a success; treating it as one would feed the parser a reply the
        // model never finished.
        _ => QueryOutcome::UnexpectedStop,
    }
}

/// The recon gather, run against the real filesystem.
pub struct FsProposeGather {
    /// Repository root.
    pub root: std::path::PathBuf,
    /// The user's config directory (holds `settings.json` and `CLAUDE.md`).
    pub user_config_dir: std::path::PathBuf,
    /// This project's transcript directory.
    pub transcript_dir: std::path::PathBuf,
    /// Whether `autoMode.classifyAllShell` is active.
    pub classify_all_shell: bool,
}

impl ProposeGather for FsProposeGather {
    fn gather(&self, answers: &ProposeAnswers) -> Result<String, String> {
        // The answers decide how far the gather may reach; a producer behind a
        // closed gate is never invoked.
        let options = permission::auto_mode_pregather::gather_options_from_answers(
            Some(&answers.scope),
            Some(&answers.depth),
        );
        let producers = permission::auto_mode_io::FsReconProducers::new(
            &self.root,
            &self.user_config_dir,
            &self.transcript_dir,
            self.classify_all_shell,
        );
        Ok(build_recon_block(options, &producers).text)
    }
}

/// The gather options the answers imply, exposed for callers that need to know
/// what a run would reach before running it.
#[must_use]
pub fn gather_reach(answers: &ProposeAnswers) -> GatherOptions {
    permission::auto_mode_pregather::gather_options_from_answers(
        Some(&answers.scope),
        Some(&answers.depth),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_client::MessageDeltaPayload;

    fn text_delta(s: &str) -> Result<LlmEvent, LlmError> {
        Ok(LlmEvent::ContentBlockDelta {
            index: 0,
            delta: ContentDelta::TextDelta {
                text: s.to_string(),
            },
        })
    }
    fn stop(reason: &str) -> Result<LlmEvent, LlmError> {
        Ok(LlmEvent::MessageDelta {
            delta: MessageDeltaPayload {
                stop_reason: Some(reason.to_string()),
                stop_details: None,
            },
            usage: None,
        })
    }

    #[test]
    fn a_completed_reply_yields_its_accumulated_text() {
        let outcome = collect_propose_reply(vec![
            text_delta("{\"environment\":"),
            text_delta("[\"laptop\"]}"),
            stop("end_turn"),
        ]);
        assert_eq!(
            outcome,
            QueryOutcome::Text("{\"environment\":[\"laptop\"]}".to_string())
        );
    }

    #[test]
    fn a_truncated_reply_is_not_handed_over_as_a_short_one() {
        // Its prefix would parse as malformed JSON and surface as a confusing
        // parse error instead of the clear "cut off" message.
        for reason in ["max_tokens", "length"] {
            let outcome =
                collect_propose_reply(vec![text_delta("{\"environment\":"), stop(reason)]);
            assert_eq!(outcome, QueryOutcome::Truncated, "reason={reason}");
        }
    }

    #[test]
    fn a_refusal_and_an_unknown_stop_are_distinguished() {
        assert_eq!(
            collect_propose_reply(vec![stop("refusal")]),
            QueryOutcome::Refused
        );
        assert_eq!(
            collect_propose_reply(vec![stop("tool_use")]),
            QueryOutcome::UnexpectedStop
        );
    }

    #[test]
    fn a_stream_that_never_reported_a_stop_reason_is_not_a_success() {
        // Otherwise an interrupted stream's partial text would be parsed as if
        // the model had finished.
        assert_eq!(
            collect_propose_reply(vec![text_delta("{\"environment\":")]),
            QueryOutcome::UnexpectedStop
        );
        assert_eq!(collect_propose_reply(vec![]), QueryOutcome::UnexpectedStop);
    }

    #[test]
    fn a_transport_error_fails_the_call() {
        let outcome = collect_propose_reply(vec![
            text_delta("partial"),
            Err(LlmError::InvalidRequest {
                message: "boom".to_string(),
            }),
        ]);
        assert!(matches!(outcome, QueryOutcome::Failed(_)));
    }

    #[test]
    fn non_text_deltas_do_not_contaminate_the_document() {
        // Thinking blocks must not end up inside the JSON handed to the parser.
        let outcome = collect_propose_reply(vec![
            Ok(LlmEvent::ContentBlockDelta {
                index: 0,
                delta: ContentDelta::ThinkingDelta {
                    thinking: "let me consider".to_string(),
                },
            }),
            text_delta("{\"ok\":true}"),
            stop("end_turn"),
        ]);
        assert_eq!(outcome, QueryOutcome::Text("{\"ok\":true}".to_string()));
    }

    #[test]
    fn the_gather_reach_follows_the_answers() {
        let conservative = ProposeAnswers {
            posture: "enterprise".into(),
            scope: "project".into(),
            depth: "here".into(),
        };
        assert_eq!(gather_reach(&conservative), GatherOptions::default());

        let broad = ProposeAnswers {
            posture: "enterprise".into(),
            scope: "all".into(),
            depth: "both".into(),
        };
        let reach = gather_reach(&broad);
        assert!(reach.all_projects && reach.shell_history && reach.home_repos);
    }

    #[test]
    fn the_gather_runs_the_producers_against_a_real_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let config = dir.path().join("config");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(root.join("CLAUDE.md"), "project rules").unwrap();

        let gather = FsProposeGather {
            root: root.clone(),
            user_config_dir: config,
            transcript_dir: root.join(".transcripts"),
            classify_all_shell: false,
        };
        let block = gather
            .gather(&ProposeAnswers {
                posture: "enterprise".into(),
                scope: "project".into(),
                depth: "here".into(),
            })
            .unwrap();

        assert!(block.contains("## Pre-gathered recon"));
        assert!(block.contains("\"project rules\""));
        // Declined reaches are withheld, not silently empty.
        assert!(block.contains("_NOT GATHERED"));
    }
}
