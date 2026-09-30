package com.lingxi.code.notify

import com.lingxi.code.model.NotifConfig

/**
 * When a system notification is warranted.
 *
 * The policy is Claude Code 2.1.270's, copied rather than invented, so that the
 * three GUI clients and the CLI do not end up with four different answers.
 * Verified against the shipped oracle binary, not inferred:
 *
 *  - `RJe = 6000` — a permission prompt must sit unanswered this long before
 *    its notification fires. Upstream arms a `setTimeout` when the prompt
 *    appears and `clearTimeout`s it in a `finally`, so a prompt answered
 *    promptly notifies nothing at all.
 *  - `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs = 60000` — how long a
 *    session must sit idle AFTER a turn ends before it says it is waiting.
 *
 * Two consequences are worth stating because both are counter-intuitive and
 * both are deliberate:
 *
 *  1. **A finished turn does not notify.** It ARMS a timer. If the user comes
 *     back inside the threshold, nothing fires — upstream's `class See`
 *     re-checks `getLastInteractionTime() > lastQueryCompletionTime` at fire
 *     time rather than trusting the timer.
 *  2. **The port keeps its own message wording.** Upstream's terminal has no
 *     per-session context, so its one idle message is the generic "Claude is
 *     waiting for your input"; this client already knows whether the turn
 *     succeeded or failed and says so. The TIMING and the GATES are upstream's;
 *     only the string is ours, and it is the one already translated into all
 *     five locales as `chat_background_{completed,failed}_*`.
 *
 * Deliberately NOT mirrored: `preferredNotifChannel` (every value but
 * `notifications_disabled` names a terminal escape sequence, which an Android
 * notification has no analogue for — [NotifConfig.enabled] is the GUI
 * equivalent of that one meaningful distinction) and `agentPushNotifEnabled`
 * (it gates the `PushNotification` tool, which is registered-but-disabled in
 * this port, so the toggle could never do anything).
 */
object NotificationPolicy {
    /** Upstream `DEFAULT_GLOBAL_CONFIG.messageIdleNotifThresholdMs`. */
    const val DEFAULT_IDLE_NOTIF_THRESHOLD_MS: Long = 60_000

    /** Upstream `RJe`. Not user-configurable upstream, so it is a constant here too. */
    const val PERMISSION_PROMPT_NOTIFY_DELAY_MS: Long = 6_000

    /** A mistyped threshold must not become "instant banner on every turn". */
    const val MIN_IDLE_THRESHOLD_MS: Long = 5_000

    /** A mistyped threshold must not silently disable the notification either —
     * that is what [NotifConfig.enabled] is for, and it says so in the UI. */
    const val MAX_IDLE_THRESHOLD_MS: Long = 3_600_000

    fun clampIdleThreshold(raw: Long): Long =
        raw.coerceIn(MIN_IDLE_THRESHOLD_MS, MAX_IDLE_THRESHOLD_MS)
}

/**
 * Upstream's `notificationType` discriminator, kept verbatim. The port's CLI
 * already uses these exact strings for the `Notification` hook
 * (`apps/cli/host/src/idle_notify.rs`,
 * `permission_prompt_notify.rs`), so a third spelling would be the drift this
 * file exists to prevent.
 */
enum class NotificationKind(val wireName: String) {
    IdlePrompt("idle_prompt"),
    PermissionPrompt("permission_prompt"),
    AgentNeedsInput("agent_needs_input"),
    AgentCompleted("agent_completed"),
    ScheduledRun("scheduled_run"),
}

/** Which preference gates a given kind. */
fun NotifConfig.allows(kind: NotificationKind): Boolean {
    if (!enabled) return false
    return when (kind) {
        NotificationKind.IdlePrompt -> idlePromptNotifEnabled
        NotificationKind.PermissionPrompt,
        NotificationKind.AgentNeedsInput -> inputNeededNotifEnabled
        NotificationKind.AgentCompleted -> taskCompleteNotifEnabled
        NotificationKind.ScheduledRun -> scheduledRunNotifEnabled
    }
}
