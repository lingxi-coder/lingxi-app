package com.lingxi.code.conversation

import com.lingxi.code.model.Message

/** Locate the persisted envelope even when its content projects to several prose/tool rows. */
internal fun messageLinkRenderIndex(items: List<ChatRenderItem>, messages: List<Message>, uuid: String): Int {
    val message = messages.firstOrNull { it.matchesTranscriptUuid(uuid) } ?: return -1
    val tools = message.blocks.filterIsInstance<MessageContent.Tool>().map { it.call.id }.toSet()
    return items.indexOfFirst { item ->
        when (item) {
            is ChatRenderItem.Message -> item.message.matchesTranscriptUuid(uuid)
            is ChatRenderItem.Tools -> item.calls.any { it.id in tools }
            is ChatRenderItem.Visualization -> item.messageId == message.id
            else -> false
        }
    }
}

internal fun Message.matchesTranscriptUuid(uuid: String): Boolean =
    transcriptUuid.equals(uuid, ignoreCase = true) || transcriptAliases.any { it.equals(uuid, ignoreCase = true) }
