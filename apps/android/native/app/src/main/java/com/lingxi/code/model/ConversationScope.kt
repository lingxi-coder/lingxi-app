package com.lingxi.code.model

/**
 * Which workspace the conversation engine is bound to. `Global` is the default
 * shell workspace, `Scheduled` the managed no-project task workspace,
 * `Project` a user project's workspace, and `LocalApp` a
 * local app's `apps/<id>/workspace` directory — protocol v3 made each app a
 * conversation scope with its own session catalog (`ListAppSessions`).
 *
 * PURE (no engine / Android types) so scope routing and the per-scope
 * persistence key are unit-testable on the plain JVM.
 */
sealed interface ConversationScope {
    data object Global : ConversationScope
    /** Managed no-project scheduled sessions, displayed with general chats. */
    data object Scheduled : ConversationScope
    data class Project(val projectId: String) : ConversationScope
    data class LocalApp(val appId: String) : ConversationScope
}

/**
 * The conversation capability profile currently selected for a workspace.
 *
 * Kept as the Android UI/domain spelling and converted to the shared
 * `SessionModeDto` only at the generated-binding boundary.
 */
enum class SessionMode(val wireValue: String) {
    Chat("chat"),
    Code("code"),
    ;

    /**
     * Back-compat alias for persistence helpers that talk in terms of "key"
     * rather than wire values.
     */
    val wireKey: String get() = wireValue
}

/**
 * The durable key a scope's state (last-active session, composer draft) is
 * stored under — `global` / `scheduled` / `project.<id>` / `app.<id>`.
 */
fun ConversationScope.persistenceKey(): String = when (this) {
    ConversationScope.Global -> "global"
    ConversationScope.Scheduled -> "scheduled"
    is ConversationScope.Project -> "project.$projectId"
    is ConversationScope.LocalApp -> "app.$appId"
}

/** Durable key for state kept separately per workspace and session mode. */
fun ConversationScope.sessionStateKey(mode: SessionMode): String = "${persistenceKey()}#${mode.wireKey}"

/** Inverse of [persistenceKey]; null for an unrecognized key. */
fun conversationScopeFromKey(key: String?): ConversationScope? = when {
    key == null -> null
    key == "global" -> ConversationScope.Global
    key == "scheduled" -> ConversationScope.Scheduled
    key.startsWith("project.") -> key.removePrefix("project.")
        .takeIf { it.isNotBlank() }?.let { ConversationScope.Project(it) }
    key.startsWith("app.") -> key.removePrefix("app.")
        .takeIf { it.isNotBlank() }?.let { ConversationScope.LocalApp(it) }
    else -> null
}
