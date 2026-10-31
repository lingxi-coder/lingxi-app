package com.lingxi.code.conversation

import com.lingxi.code.bindings.android.RemoteImageFfi
import com.lingxi.code.bindings.android.RemoteModelsFfi
import com.lingxi.code.bindings.android.RemoteOutgoingPromptFfi
import com.lingxi.code.bindings.android.RemotePendingFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemoteRecordFfi
import com.lingxi.code.bindings.android.RemoteSeqRecordFfi
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.android.RemoteTurnFfi
import com.lingxi.code.bindings.client.MessageBlockDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow

internal fun remoteUpdate(
    sessionId: String = "s1",
    phase: RemoteSessionPhaseFfi = RemoteSessionPhaseFfi.LIVE,
    replace: Boolean = false,
    messages: List<MessageDto> = emptyList(),
    older: List<MessageDto> = emptyList(),
    records: List<RemoteRecordFfi> = emptyList(),
    pending: RemotePendingFfi = RemotePendingFfi(emptyList(), emptyList(), emptyList()),
    running: Boolean = false,
    outgoing: List<RemoteOutgoingPromptFfi> = emptyList(),
    hasOlder: Boolean = false,
    answeredElsewhere: Boolean = false,
) = RemoteSessionUpdateFfi(
    sessionId = sessionId,
    phase = phase,
    error = null,
    replace = replace,
    messages = messages,
    olderMessages = older,
    firstMessageIndex = 0uL,
    records = records.mapIndexed { index, record -> RemoteSeqRecordFfi(index.toULong(), record) },
    pending = pending,
    turn = RemoteTurnFfi(running, null),
    controls = null,
    outgoing = outgoing,
    hasOlder = hasOlder,
    loadingOlder = false,
    answeredElsewhere = answeredElsewhere,
)

internal fun textMessage(role: String, text: String) =
    MessageDto(transcriptUuid = null, role = role, blocks = listOf(MessageBlockDto.Text(text.toClientTextDto())))

/** A [RemoteSessionPort] that replays what the test pushes and records what the source asks for. */
internal class FakeRemoteSessionPort : RemoteSessionPort {
    val updates = MutableSharedFlow<RemoteSessionUpdateFfi>(replay = 1, extraBufferCapacity = 16)
    val calls = mutableListOf<String>()
    val sentImages = mutableListOf<List<RemotePromptImageFfi>>()
    var fetchGate: CompletableDeferred<Unit>? = null

    /** The Bot that handed over a turn's prompt, by turn id; a turn without an entry has none. */
    val botNames = mutableMapOf<ULong, String>()

    override fun follow(): Flow<RemoteSessionUpdateFfi> = updates
    override suspend fun setForeground(foreground: Boolean) { calls += "foreground:$foreground" }
    override suspend fun loadOlder() { calls += "loadOlder" }
    override suspend fun fetchImage(imageRef: String): RemoteImageFfi {
        calls += "fetch:$imageRef"
        fetchGate?.await()
        return RemoteImageFfi("image/png", byteArrayOf(1, 2, 3))
    }
    override suspend fun sendPrompt(text: String, images: List<RemotePromptImageFfi>) {
        calls += "send:$text"
        sentImages += images
    }
    override suspend fun retryPrompt(promptId: String) { calls += "retry:$promptId" }
    override suspend fun dismissPrompt(promptId: String) { calls += "dismiss:$promptId" }
    override suspend fun cancelTurn() { calls += "cancel" }
    override suspend fun respondPermission(requestId: ULong, allow: Boolean) { calls += "permission:$requestId:$allow" }
    override suspend fun answerQuestion(requestId: ULong, answers: Map<String, String>) { calls += "answer:$requestId:$answers" }
    override suspend fun cancelQuestion(requestId: ULong) { calls += "cancelQuestion:$requestId" }
    override suspend fun setModel(model: String) { calls += "model:$model" }
    override suspend fun setReasoning(selection: ReasoningSelectionDto) { calls += "reasoning" }
    override suspend fun setPermissionMode(mode: RemotePermissionModeFfi) { calls += "mode:$mode" }
    override suspend fun listModels(): RemoteModelsFfi = RemoteModelsFfi(listOf("a"), "a", emptyList())
    override fun promptOriginBotName(turnId: ULong): String? = botNames[turnId]
}
