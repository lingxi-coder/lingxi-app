use super::*;

/// Reminders collected once for a single model step.
///
/// Transient reminders are re-appended when the same step rebuilds its request.
/// Task notifications are durable conversation events: they are persisted here
/// and returned separately so the caller can add them to a snapshot that was
/// captured before persistence without duplicating them on retry.
pub(crate) struct TurnReminders {
    pub(crate) transient: Vec<ConversationMessage>,
    pub(crate) task_notifications: Vec<ConversationMessage>,
}

impl ConversationOrchestrator {
    /// Collect reminder producers in their model-facing order exactly once.
    pub(crate) async fn collect_turn_reminders(&self, in_human_turn: bool) -> TurnReminders {
        let mut transient = Vec::new();

        if let Some(reminder) = self.brief_mode_reminder_message() {
            transient.push(reminder);
        }
        if let Some(reminder) = self.output_style_reminder_message().await {
            transient.push(reminder);
        }

        transient.extend(self.plan_mode_turn_messages().await);
        if let Some(reminder) = self.plan_mode_exit_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.skill_listing_reminder_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.conditional_rules_reminder_message().await {
            transient.push(reminder);
        }
        // Nested memory runs directly AFTER conditional rules, and the order is
        // load-bearing, not cosmetic: both consult the same
        // `sent_conditional_rules` set, and a `paths:`-gated rule already
        // claimed by the conditional-rules producer is skipped here. Swapping
        // the two changes WHICH mechanism reports such a rule, and therefore
        // the bytes the model sees.
        if let Some(reminder) = self.nested_memory_reminder_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.new_diagnostics_reminder_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.agent_listing_reminder_message().await {
            transient.push(reminder);
        }
        // DIVERGENCE (position, deliberate): the oracle's attachment fan-out
        // (@296520120) emits `changed_files` immediately after
        // `agent_listing_delta` and immediately BEFORE `nested_memory`. This
        // port injects `nested_memory` above — it has to, to read the
        // `sent_conditional_rules` claims noted there — so `changed_files`
        // sits directly after `agent_listing_delta` instead, which preserves
        // its order relative to everything downstream. `crate::prompt::changed_files`
        // carries the same note from the renderer's side.
        transient.extend(self.changed_files_reminder_messages().await);

        // `todo_reminder_message` already returns a system-reminder envelope.
        let todo_reminder = self.todo_reminder_message().await;
        let todo_reminder_fired = todo_reminder.is_some();
        if let Some(reminder) = todo_reminder {
            transient.push(reminder);
        }
        if let Some(reminder) = self
            .tool_search_usage_reminder_message(todo_reminder_fired)
            .await
        {
            transient.push(reminder);
        }
        if let Some(reminder) = self.async_hook_response_reminder_message().await {
            transient.push(reminder);
        }

        let task_notifications = self
            .task_notification_reminder_messages_in_turn(in_human_turn)
            .await;
        for notification in &task_notifications {
            {
                let mut session = self.session.lock().await;
                session.history.push(notification.clone());
            }
            self.persist_message_to_jsonl(notification).await;
        }

        // Task completion can enqueue memory updates, so this must follow the
        // notification drain and persistence above.
        transient.extend(self.memory_update_reminder_messages().await);
        transient.extend(self.relevant_memory_reminder_messages().await);
        if let Some(reminder) = self.skill_discovery_reminder_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.silent_turn_reminder_message().await {
            transient.push(reminder);
        }
        if let Some(reminder) = self.total_tokens_reminder_message().await {
            transient.push(reminder);
        }

        TurnReminders {
            transient,
            task_notifications,
        }
    }
}
