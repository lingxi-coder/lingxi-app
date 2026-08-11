package com.lingxi.code.model

/**
 * Which workspace the conversation engine is bound to. `Global` is the default
 * shell workspace, `Project` a user project's workspace, and `LocalApp` a
 * local app's `apps/<id>/workspace` directory — protocol v3 made each app a
 * conversation scope with its own session catalog (`ListAppSessions`).
 *
 * PURE (no engine / Android types) so scope routing and the per-scope
 * persistence key are unit-testable on the plain JVM.
 */
sealed interface ConversationScope {
    data object Global : ConversationScope
    data class Project(val projectId: String) : ConversationScope
    data class LocalApp(val appId: String) : ConversationScope
}

/**
 * The durable key a scope's state (last-active session, composer draft) is
 * stored under — `global` / `project.<id>` / `app.<id>`.
 */
fun ConversationScope.persistenceKey(): String = when (this) {
    ConversationScope.Global -> "global"
    is ConversationScope.Project -> "project.$projectId"
    is ConversationScope.LocalApp -> "app.$appId"
}

/** Inverse of [persistenceKey]; null for an unrecognized key. */
fun conversationScopeFromKey(key: String?): ConversationScope? = when {
    key == null -> null
    key == "global" -> ConversationScope.Global
    key.startsWith("project.") -> key.removePrefix("project.")
        .takeIf { it.isNotBlank() }?.let { ConversationScope.Project(it) }
    key.startsWith("app.") -> key.removePrefix("app.")
        .takeIf { it.isNotBlank() }?.let { ConversationScope.LocalApp(it) }
    else -> null
}
