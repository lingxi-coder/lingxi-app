package com.lingxi.code.conversation

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext

/** Native construction can hash a rootfs and cannot be interrupted halfway.
 * Keep its returned handle until the caller can adopt it, including cancellation.
 */
internal suspend fun prepareConversationSource(
    borrowed: ConversationSource? = null,
    dispatcher: CoroutineDispatcher = Dispatchers.IO,
    create: () -> ConversationSource,
): ConversationSource {
    var prepared: ConversationSource? = null
    try {
        withContext(dispatcher + NonCancellable) { prepared = create() }
        currentCoroutineContext().ensureActive()
        return checkNotNull(prepared).also { prepared = null }
    } finally {
        prepared?.takeIf { it !== borrowed }?.let { abandoned ->
            withContext(dispatcher + NonCancellable) { abandoned.close() }
        }
    }
}
