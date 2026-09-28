package com.lingxi.code.conversation

import android.content.Context

/**
 * Resolves a localized string for conversation-package code that runs OUTSIDE a
 * `@Composable` body — [ChatViewModel], [ConversationSource] and the pure
 * top-level mappers below ([clientEventToReply], [userFacingEngineError],
 * [mapReplyStream], [messageDtoText]) — and therefore cannot call
 * `stringResource()`.
 *
 * [fallback] is always the exact zh-Hans base-locale copy for [id], passed in at
 * the call site right next to the resource id. [DefaultConversationStrings]
 * (the parameter default everywhere this is threaded through) returns [fallback]
 * verbatim — formatted with [args] when present — which is why the many JVM unit
 * tests that construct [ChatViewModel] / call these mappers directly, with no
 * Android `Context` at all, keep asserting the same literal Chinese copy without
 * being touched. The production implementation ([conversationStrings]) ignores
 * [fallback] and resolves the REAL localized text through
 * [Context.getString], so the app actually renders in the user's selected
 * language, not just in tests.
 */
fun interface ConversationStrings {
    fun resolve(id: Int, fallback: String, vararg args: Any): String
}

/** Test/no-Context fallback: the literal zh-Hans copy, `String.format`-ed. */
val DefaultConversationStrings = ConversationStrings { _, fallback, args ->
    if (args.isEmpty()) fallback else String.format(java.util.Locale.getDefault(), fallback, *args)
}

/** Production resolver: real localized text via the app's (locale-wrapped) [Context]. */
fun conversationStrings(context: Context): ConversationStrings =
    ConversationStrings { id, _, args -> context.getString(id, *args) }
