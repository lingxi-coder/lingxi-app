import Foundation

/// Persistence for ``NotifConfig``.
///
/// `UserDefaults` directly, the way ``VoiceRuntimeConfiguration`` does, rather
/// than through `SettingsStore` — the reader is
/// ``ConversationBackgroundAlertController``, which decides whether to post at
/// the moment a timer fires, possibly with no settings screen ever having been
/// opened this launch.
///
/// Key names are upstream Claude Code's setting names, matching `NotifConfig`'s
/// fields; see ``NotificationPolicy`` for why the vocabulary is borrowed.
enum NotificationPreferencesStore {
    private enum Keys {
        static let enabled = "notifications.enabled"
        static let idlePrompt = "notifications.idlePromptNotifEnabled"
        static let inputNeeded = "notifications.inputNeededNotifEnabled"
        static let taskComplete = "notifications.taskCompleteNotifEnabled"
        static let scheduledRun = "notifications.scheduledRunNotifEnabled"
        static let idleThresholdMs = "notifications.messageIdleNotifThresholdMs"
    }

    /// An absent key reads as its DEFAULT, never as `false`: a wiped or
    /// half-written store must not silently turn notifications off, which is
    /// the one failure the settings screen cannot show the user. `bool(forKey:)`
    /// returns `false` for a missing key, so every read is existence-checked.
    static func load(from defaults: UserDefaults = .standard) -> NotifConfig {
        var config = NotifConfig()
        func bool(_ key: String, _ fallback: Bool) -> Bool {
            defaults.object(forKey: key) != nil ? defaults.bool(forKey: key) : fallback
        }
        config.enabled = bool(Keys.enabled, config.enabled)
        config.idlePromptNotifEnabled = bool(Keys.idlePrompt, config.idlePromptNotifEnabled)
        config.inputNeededNotifEnabled = bool(Keys.inputNeeded, config.inputNeededNotifEnabled)
        config.taskCompleteNotifEnabled = bool(Keys.taskComplete, config.taskCompleteNotifEnabled)
        config.scheduledRunNotifEnabled = bool(Keys.scheduledRun, config.scheduledRunNotifEnabled)
        config.messageIdleNotifThresholdMs = NotificationPolicy.clampIdleThreshold(
            defaults.object(forKey: Keys.idleThresholdMs) != nil
                ? defaults.integer(forKey: Keys.idleThresholdMs)
                : config.messageIdleNotifThresholdMs
        )
        return config
    }

    /// Whole-object write, matching how desktop and Android persist theirs.
    static func save(_ config: NotifConfig, to defaults: UserDefaults = .standard) {
        defaults.set(config.enabled, forKey: Keys.enabled)
        defaults.set(config.idlePromptNotifEnabled, forKey: Keys.idlePrompt)
        defaults.set(config.inputNeededNotifEnabled, forKey: Keys.inputNeeded)
        defaults.set(config.taskCompleteNotifEnabled, forKey: Keys.taskComplete)
        defaults.set(config.scheduledRunNotifEnabled, forKey: Keys.scheduledRun)
        defaults.set(
            NotificationPolicy.clampIdleThreshold(config.messageIdleNotifThresholdMs),
            forKey: Keys.idleThresholdMs
        )
    }
}
