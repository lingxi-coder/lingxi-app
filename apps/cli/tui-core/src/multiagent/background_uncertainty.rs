//! User-visible reporting for uncertain background task transitions.

use lingxi_core::host::task_registry::TaskBackgroundBatchOutcome;

/// Describe separately an unknown task set and unknown results for captured tasks.
pub fn background_uncertainty_message(outcome: TaskBackgroundBatchOutcome) -> Option<String> {
    let mut message = String::new();
    if outcome.capture_unknown {
        message.push_str("Could not determine the complete set of tasks to background.");
    }
    if outcome.unknown != 0 {
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(&format!(
            "Could not confirm backgrounding for {} task(s).",
            outcome.unknown
        ));
    }
    (!message.is_empty()).then_some(message)
}

/// Suppress repeated notices during polling while tracking both uncertainties.
#[derive(Default)]
pub struct BackgroundUncertaintyTracker {
    capture_warned: bool,
    tasks_warned: bool,
}

impl BackgroundUncertaintyTracker {
    pub fn next_message(&mut self, outcome: TaskBackgroundBatchOutcome) -> Option<String> {
        let newly_uncertain = TaskBackgroundBatchOutcome {
            backgrounded: 0,
            unknown: if self.tasks_warned {
                0
            } else {
                outcome.unknown
            },
            capture_unknown: outcome.capture_unknown && !self.capture_warned,
        };
        self.capture_warned = outcome.capture_unknown;
        self.tasks_warned = outcome.unknown != 0;
        background_uncertainty_message(newly_uncertain)
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_set_uncertainty_does_not_invent_a_task_count() {
        let capture = TaskBackgroundBatchOutcome {
            capture_unknown: true,
            ..Default::default()
        };
        let message = background_uncertainty_message(capture).unwrap();
        assert_eq!(
            message,
            "Could not determine the complete set of tasks to background."
        );
        let both = TaskBackgroundBatchOutcome {
            unknown: 2,
            ..capture
        };
        let message = background_uncertainty_message(both).unwrap();
        assert!(message.contains("complete set of tasks"));
        assert!(message.contains("backgrounding for 2 task(s)"));
        assert!(background_uncertainty_message(Default::default()).is_none());
    }

    #[test]
    fn polling_reports_each_new_kind_of_uncertainty_once() {
        let mut tracker = BackgroundUncertaintyTracker::default();
        let task_unknown = TaskBackgroundBatchOutcome {
            unknown: 1,
            ..Default::default()
        };
        assert!(tracker
            .next_message(task_unknown)
            .unwrap()
            .contains("1 task"));
        assert!(tracker.next_message(task_unknown).is_none());
        let both = TaskBackgroundBatchOutcome {
            capture_unknown: true,
            ..task_unknown
        };
        let message = tracker.next_message(both).unwrap();
        assert!(message.contains("complete set of tasks"));
        assert!(!message.contains("1 task"));
        assert!(tracker.next_message(both).is_none());
        tracker.reset();
        let message = tracker.next_message(both).unwrap();
        assert!(message.contains("complete set of tasks"));
        assert!(message.contains("1 task"));
    }
}
