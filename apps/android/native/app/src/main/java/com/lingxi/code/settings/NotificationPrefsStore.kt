package com.lingxi.code.settings

import android.content.Context
import com.lingxi.code.model.NotifConfig
import com.lingxi.code.notify.NotificationPolicy

/**
 * Persistence for [NotifConfig], using a process-wide SharedPreferences file with synchronous
 * `load()` / `save()`.
 *
 * Synchronous on purpose. The reader is
 * [com.lingxi.code.conversation.AndroidConversationBackgroundExecution], which
 * decides whether to post at the moment a timer fires — possibly with the
 * Activity long gone and no coroutine scope to suspend in.
 *
 * Key names are upstream Claude Code's setting names, matching [NotifConfig]'s
 * fields — see [NotificationPolicy] for why the vocabulary is borrowed.
 */
class NotificationPrefsStore(context: Context) {
    private val preferences = context.applicationContext.getSharedPreferences(
        PREFERENCES_NAME,
        Context.MODE_PRIVATE,
    )

    /**
     * An absent key reads as its DEFAULT, never as `false`: a wiped or
     * half-written store must not silently turn notifications off, which is the
     * one failure the settings screen cannot show the user.
     */
    fun load(): NotifConfig {
        val defaults = NotifConfig()
        return NotifConfig(
            enabled = preferences.getBoolean(KEY_ENABLED, defaults.enabled),
            idlePromptNotifEnabled = preferences.getBoolean(KEY_IDLE_PROMPT, defaults.idlePromptNotifEnabled),
            inputNeededNotifEnabled = preferences.getBoolean(KEY_INPUT_NEEDED, defaults.inputNeededNotifEnabled),
            taskCompleteNotifEnabled = preferences.getBoolean(KEY_TASK_COMPLETE, defaults.taskCompleteNotifEnabled),
            scheduledRunNotifEnabled = preferences.getBoolean(KEY_SCHEDULED_RUN, defaults.scheduledRunNotifEnabled),
            messageIdleNotifThresholdMs = NotificationPolicy.clampIdleThreshold(
                preferences.getLong(KEY_IDLE_THRESHOLD_MS, defaults.messageIdleNotifThresholdMs),
            ),
        )
    }

    /** Whole-object write, matching how desktop and iOS persist theirs. */
    fun save(config: NotifConfig) {
        preferences.edit()
            .putBoolean(KEY_ENABLED, config.enabled)
            .putBoolean(KEY_IDLE_PROMPT, config.idlePromptNotifEnabled)
            .putBoolean(KEY_INPUT_NEEDED, config.inputNeededNotifEnabled)
            .putBoolean(KEY_TASK_COMPLETE, config.taskCompleteNotifEnabled)
            .putBoolean(KEY_SCHEDULED_RUN, config.scheduledRunNotifEnabled)
            .putLong(
                KEY_IDLE_THRESHOLD_MS,
                NotificationPolicy.clampIdleThreshold(config.messageIdleNotifThresholdMs),
            )
            .apply()
    }

    private companion object {
        const val PREFERENCES_NAME = "notification_settings"
        const val KEY_ENABLED = "enabled"
        const val KEY_IDLE_PROMPT = "idlePromptNotifEnabled"
        const val KEY_INPUT_NEEDED = "inputNeededNotifEnabled"
        const val KEY_TASK_COMPLETE = "taskCompleteNotifEnabled"
        const val KEY_SCHEDULED_RUN = "scheduledRunNotifEnabled"
        const val KEY_IDLE_THRESHOLD_MS = "messageIdleNotifThresholdMs"
    }
}
