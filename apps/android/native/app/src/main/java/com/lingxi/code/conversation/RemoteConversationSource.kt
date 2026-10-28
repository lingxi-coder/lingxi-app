package com.lingxi.code.conversation

import com.lingxi.code.R
import com.lingxi.code.bindings.android.RemoteImageFfi
import com.lingxi.code.bindings.android.RemoteModelsFfi
import com.lingxi.code.bindings.android.RemoteOutgoingStatusFfi
import com.lingxi.code.bindings.android.RemotePermissionModeFfi
import com.lingxi.code.bindings.android.RemotePromptImageFfi
import com.lingxi.code.bindings.android.RemotePromptOriginFfi
import com.lingxi.code.bindings.android.RemoteRecordFfi
import com.lingxi.code.bindings.android.RemoteSessionErrorFfi
import com.lingxi.code.bindings.android.RemoteSessionPhaseFfi
import com.lingxi.code.bindings.android.RemoteSessionUpdateFfi
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.ConversationControlsDto
import com.lingxi.code.bindings.client.ImageRefDto
import com.lingxi.code.bindings.client.MessageDto
import com.lingxi.code.bindings.client.PermissionKindDto
import com.lingxi.code.bindings.client.PermissionResponseDto
import com.lingxi.code.bindings.client.ReasoningSelectionDto
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.ReasoningControl
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.channelFlow
import kotlinx.coroutines.launch
import java.util.Base64
import java.util.concurrent.ConcurrentHashMap

/** The calls a followed session makes on the Remote Control client; tests substitute it. */
interface RemoteSessionPort {
    /** The session's updates, starting with the state held so far; following ends with the collection. */
    fun follow(): Flow<RemoteSessionUpdateFfi>
    suspend fun setForeground(foreground: Boolean)
    suspend fun loadOlder()
    suspend fun fetchImage(imageRef: String): RemoteImageFfi
    suspend fun sendPrompt(text: String, images: List<RemotePromptImageFfi>)
    suspend fun retryPrompt(promptId: String)
    suspend fun dismissPrompt(promptId: String)
    suspend fun cancelTurn()
    suspend fun respondPermission(requestId: ULong, allow: Boolean)
    suspend fun answerQuestion(requestId: ULong, answers: Map<String, String>)
    suspend fun cancelQuestion(requestId: ULong)
    suspend fun setModel(model: String)
    suspend fun setReasoning(selection: ReasoningSelectionDto)
    suspend fun setPermissionMode(mode: RemotePermissionModeFfi)
    suspend fun listModels(): RemoteModelsFfi

    /**
     * The name of the Bot that handed over the prompt of turn [turnId], or null when the connection
     * knows of none (the prompt came from a schedule, or the Bot is not known any more).
     */
    fun promptOriginBotName(turnId: ULong): String?
}

/** The parts of a followed session the screen shows besides the transcript. */
data class RemoteSessionMeta(
    val phase: RemoteSessionPhaseFfi = RemoteSessionPhaseFfi.LOADING,
    val error: RemoteSessionErrorFfi? = null,
    val hasOlder: Boolean = false,
    val loadingOlder: Boolean = false,
    val controls: ConversationControlsDto? = null,
)

/** Largest picture and most pictures the desktop accepts in a prompt. */
internal const val REMOTE_MAX_PROMPT_IMAGES = 4
internal const val REMOTE_MAX_PROMPT_IMAGE_BYTES = 2 * 1024 * 1024

/**
 * A desktop session followed from the phone. It lowers the session's
 * [RemoteSessionUpdateFfi] deltas to [ExternalFeedItem]s, which [ChatViewModel]
 * renders with the reducers a local engine uses, and turns the user's actions
 * (send, stop, answer, change model) into calls on [port].
 *
 * Only what the desktop allows is offered: a permission is answered "allow once"
 * or "deny", never "always allow", and the permission mode never becomes bypass.
 */
class RemoteConversationSource(
    private val port: RemoteSessionPort,
    private val thisDeviceName: String,
    private val strings: ConversationStrings = DefaultConversationStrings,
) : ConversationSource, ExternalTurnConversationSource {
    private val mutableMeta = MutableStateFlow(RemoteSessionMeta())
    private val mutablePermission = MutableStateFlow<PermissionPromptState?>(null)
    private val mutableModels = MutableStateFlow(EngineModelState())

    /** Images already fetched, by reference: a new snapshot re-lists the prompts that carry them. */
    private val fetchedImages = ConcurrentHashMap<String, ImageRefDto>()
    private val imageLock = Any()
    private var imageGeneration = 0L

    /** Phase, error, paging and controls of the session, current as of the last update. */
    val meta: StateFlow<RemoteSessionMeta> = mutableMeta.asStateFlow()

    override val pendingPermission: StateFlow<PermissionPromptState?> = mutablePermission.asStateFlow()
    override val modelState: StateFlow<EngineModelState> = mutableModels.asStateFlow()

    override val externalFeed: Flow<ExternalFeedItem> = channelFlow {
        val mapper = RemoteFeedMapper(strings, thisDeviceName) { port.promptOriginBotName(it) }
        var modelsRequested = false
        port.follow().collect { incoming ->
            if (incoming.phase == RemoteSessionPhaseFfi.LOADING) synchronized(imageLock) {
                imageGeneration++
                fetchedImages.clear()
            }
            val update = hydrateMessageImages(incoming)
            mutableMeta.value = RemoteSessionMeta(
                phase = update.phase,
                error = update.error,
                hasOlder = update.hasOlder,
                loadingOlder = update.loadingOlder,
                controls = update.controls,
            )
            mutablePermission.value = mapper.permissionPrompt(update)
            applyModelEvents(update)
            mapper.map(update).forEach { send(it) }
            if (update.phase == RemoteSessionPhaseFfi.LIVE && !modelsRequested) {
                modelsRequested = true
                launch { runCatching { port.listModels() }.getOrNull()?.let(::applyModelList) }
            }
            for ((turnId, images) in mapper.imageRefs(update)) {
                val generation = synchronized(imageLock) { imageGeneration }
                launch {
                    val fetched = images.mapNotNull { ref ->
                        fetchImage(ref)
                    }
                    if (fetched.isNotEmpty() && synchronized(imageLock) { generation == imageGeneration }) {
                        send(ExternalFeedItem.PromptImages(turnId, fetched))
                    }
                }
            }
        }
    }

    /** Snapshot/history/message-complete URLs are opaque image.get references, not data URLs. */
    private suspend fun hydrateMessageImages(update: RemoteSessionUpdateFfi): RemoteSessionUpdateFfi = update.copy(
        messages = update.messages.map { hydrateMessage(it) },
        olderMessages = update.olderMessages.map { hydrateMessage(it) },
        records = update.records.map { item ->
            val record = item.record as? RemoteRecordFfi.Event
            val event = record?.event as? ClientEvent.MessageComplete
            if (event?.message == null) item else item.copy(
                record = record.copy(event = event.copy(message = hydrateMessage(event.message))),
            )
        },
    )

    private suspend fun hydrateMessage(message: MessageDto): MessageDto = message.copy(
        images = message.images.mapNotNull { image ->
            if (image.url.startsWith("data:")) image else fetchImage(image.url)?.let {
                image.copy(mediaType = it.mediaType, url = "data:${it.mediaType};base64,${it.base64}")
            }
        },
    )

    private suspend fun fetchImage(ref: String): ImageRefDto? {
        val generation = synchronized(imageLock) {
            fetchedImages[ref]?.let { return it }
            imageGeneration
        }
        val image = try {
            port.fetchImage(ref).let { ImageRefDto(it.mediaType, Base64.getEncoder().encodeToString(it.bytes)) }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            return null
        }
        return synchronized(imageLock) {
            if (generation != imageGeneration) null else image.also { fetchedImages[ref] = it }
        }
    }

    /** Controls are always current; the model list and model changes arrive as records. */
    private fun applyModelEvents(update: RemoteSessionUpdateFfi) {
        var state = mutableModels.value
        update.controls?.let { controls ->
            state = reduceModelEvent(state.copy(active = controls.qualifiedModel), ClientEvent.ConversationControlsChanged(controls))
        }
        for (item in update.records) {
            val event = (item.record as? RemoteRecordFfi.Event)?.event ?: continue
            state = reduceModelEvent(state, event)
        }
        mutableModels.value = state
    }

    private fun applyModelList(models: RemoteModelsFfi) {
        mutableModels.value = reduceModelEvent(
            mutableModels.value,
            ClientEvent.ModelList(models.models, models.current, null, models.details),
        )
    }

    override suspend fun sendExternalPrompt(text: String, images: List<ImageRefDto>) {
        require(images.size <= REMOTE_MAX_PROMPT_IMAGES) { "at most $REMOTE_MAX_PROMPT_IMAGES images" }
        port.sendPrompt(
            text,
            images.map {
                val bytes = Base64.getMimeDecoder().decode(it.base64)
                require(bytes.size <= REMOTE_MAX_PROMPT_IMAGE_BYTES) { "image over 2 MiB" }
                RemotePromptImageFfi(it.mediaType, bytes)
            },
        )
    }

    override suspend fun retryExternalPrompt(promptId: String) = port.retryPrompt(promptId)

    override suspend fun dismissExternalPrompt(promptId: String) = port.dismissPrompt(promptId)

    override suspend fun cancel() = port.cancelTurn()

    override suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) {
        // The desktop turns "always allow" down; never ask it to.
        require(response == PermissionResponseDto.ALLOW_ONCE) { "only allow once is available remotely" }
        port.respondPermission(requestId, allow = true)
    }

    override suspend fun denyPermission(requestId: ULong) = port.respondPermission(requestId, allow = false)

    override suspend fun submitClientCommand(command: ClientCommand) {
        when (command) {
            is ClientCommand.AnswerAskUserQuestion -> port.answerQuestion(
                command.requestId,
                command.answers.pairs.associate { it.question.displayText() to it.answer.displayText() },
            )
            is ClientCommand.CancelAskUserQuestion -> port.cancelQuestion(command.requestId)
            // Listings and the rest belong to a local engine; there is nothing to ask the desktop.
            else -> Unit
        }
    }

    override suspend fun setModel(id: String) = port.setModel(id)

    override suspend fun setReasoningSelection(id: String) = port.setReasoning(ReasoningControl.selection(id))

    /** [mode] is a permission mode id (`default`, `acceptEdits`, `plan`, `auto`, `dontAsk`). */
    override suspend fun setPermissionMode(mode: String) {
        port.setPermissionMode(remotePermissionMode(mode) ?: throw IllegalArgumentException("mode $mode is not available remotely"))
    }

    suspend fun loadOlder() = port.loadOlder()

    suspend fun setForeground(foreground: Boolean) = port.setForeground(foreground)
}

/** The permission modes a remote device may select; bypass is never one of them. */
internal fun remotePermissionMode(id: String): RemotePermissionModeFfi? = when (id) {
    "default" -> RemotePermissionModeFfi.DEFAULT
    "acceptEdits" -> RemotePermissionModeFfi.ACCEPT_EDITS
    "plan" -> RemotePermissionModeFfi.PLAN
    "auto" -> RemotePermissionModeFfi.AUTO
    "dontAsk" -> RemotePermissionModeFfi.DONT_ASK
    else -> null
}

/**
 * Lowers one [RemoteSessionUpdateFfi] to the items [ChatViewModel] reduces. Pure
 * apart from what it remembers of the prompts it has seen (to fetch their images
 * once): `replace` becomes a new base, older history is prepended, records are
 * appended in order, and the meta fields (pending actions, turn, readiness) follow
 * last so they win over what the records implied.
 */
class RemoteFeedMapper(
    private val strings: ConversationStrings,
    private val thisDeviceName: String,
    /** Names the Bot that handed over the prompt of a turn, if one did: its prompts still arrive as scheduled ones. */
    private val botNameOf: (turnId: ULong) -> String? = { null },
) {
    fun map(update: RemoteSessionUpdateFfi): List<ExternalFeedItem> {
        val items = mutableListOf<ExternalFeedItem>()
        // A load may end in a new base: until it does, what is held stays as it is.
        if (update.phase == RemoteSessionPhaseFfi.LOADING) {
            items += ExternalFeedItem.Readiness(false, strings.resolve(R.string.remote_session_loading, "正在加载会话…"))
            return items
        }
        if (update.replace) {
            items += ExternalFeedItem.Base(
                ActivatedSession(
                    sessionId = update.sessionId,
                    transcript = transcriptFromDtos(update.messages, strings),
                    kind = SessionActivationKind.Resumed,
                ),
                title = null,
            )
        } else if (update.olderMessages.isNotEmpty()) {
            items += ExternalFeedItem.OlderMessages(transcriptFromDtos(update.olderMessages, strings))
        }
        val omitted = mutableListOf<String>()
        for (item in update.records) {
            when (val record = item.record) {
                is RemoteRecordFfi.Event -> if (!record.event.isHandledByMeta()) items += ExternalFeedItem.Event(record.event)
                is RemoteRecordFfi.Prompt -> items += ExternalFeedItem.UserPrompt(
                    turnId = record.turnId.toLong(),
                    text = record.text,
                    imageCount = record.images.size,
                    originLabel = originLabel(record.origin, record.deviceName, record.turnId),
                )
                is RemoteRecordFfi.Omitted -> omitted += record.eventType.orEmpty()
                // Pending actions come from the meta fields; an unknown kind has nothing to show.
                is RemoteRecordFfi.PermissionRequest,
                is RemoteRecordFfi.ComputerAccessRequest,
                is RemoteRecordFfi.Unsupported,
                -> Unit
            }
        }
        items += ExternalFeedItem.PendingQuestions(update.pending.questions)
        items += ExternalFeedItem.Turn(update.turn.running)
        items += ExternalFeedItem.Outgoing(
            update.outgoing.map {
                ExternalOutgoingPrompt(it.id, it.text, it.images, failed = it.status == RemoteOutgoingStatusFfi.FAILED)
            },
        )
        items += readiness(update)
        if (!update.replace && omitted.isNotEmpty()) {
            items += ExternalFeedItem.Notice(strings.resolve(R.string.remote_omitted_note, "此处有一条内容过大，未同步到远程。"))
        }
        if (update.answeredElsewhere) {
            items += ExternalFeedItem.Notice(strings.resolve(R.string.remote_answered_elsewhere, "这个请求已在别处处理。"))
        }
        return items
    }

    /** Images of prompts newly seen in [update], to fetch: turn id to image refs. */
    fun imageRefs(update: RemoteSessionUpdateFfi): List<Pair<Long, List<String>>> =
        update.records.mapNotNull { item ->
            val prompt = item.record as? RemoteRecordFfi.Prompt ?: return@mapNotNull null
            if (prompt.images.isEmpty()) null else prompt.turnId.toLong() to prompt.images.map { it.imageRef }
        }

    /**
     * The head permission request as a prompt that offers allow once and deny only.
     * A request to switch to bypass mode can only be approved on the desktop, so it is not shown.
     */
    fun permissionPrompt(update: RemoteSessionUpdateFfi): PermissionPromptState? =
        update.pending.permissions.firstOrNull { it.kind !is PermissionKindDto.BypassPermissionsMode }?.let {
            permissionRequestToPrompt(it, strings).copy(suppressAlwaysAllowRule = true, autoModePrompt = null)
        }

    private fun readiness(update: RemoteSessionUpdateFfi): ExternalFeedItem.Readiness = when (update.phase) {
        RemoteSessionPhaseFfi.LIVE -> ExternalFeedItem.Readiness(
            ready = true,
            statusLine = when {
                update.pending.permissions.any { it.kind is PermissionKindDto.BypassPermissionsMode } ->
                    strings.resolve(R.string.remote_bypass_desktop_only, "要切换到“绕过权限”模式，只能在电脑上批准。")
                update.pending.computerAccess.isNotEmpty() ->
                    strings.resolve(R.string.remote_computer_access_note, "电脑正在请求额外的访问权限。请在电脑上处理。")
                else -> null
            },
        )
        RemoteSessionPhaseFfi.OFFLINE -> ExternalFeedItem.Readiness(
            false, strings.resolve(R.string.remote_session_offline, "已离线，显示的是最后收到的内容。"),
        )
        RemoteSessionPhaseFfi.CLOSED -> ExternalFeedItem.Readiness(
            false, strings.resolve(R.string.remote_session_closed, "这个会话已不再共享。"),
        )
        RemoteSessionPhaseFfi.ERROR -> ExternalFeedItem.Readiness(false, sessionErrorText(update.error))
        RemoteSessionPhaseFfi.LOADING -> ExternalFeedItem.Readiness(
            false, strings.resolve(R.string.remote_session_loading, "正在加载会话…"),
        )
    }

    private fun sessionErrorText(error: RemoteSessionErrorFfi?): String = when (error?.code) {
        "unknown_session" -> strings.resolve(R.string.remote_err_unknown_session, "找不到这个会话。")
        "not_remote_enabled" -> strings.resolve(R.string.remote_err_not_remote_enabled, "这个会话没有开启远程访问。")
        "capacity" -> strings.resolve(R.string.remote_err_capacity, "电脑上同时打开的远程会话已达上限。")
        "forbidden" -> strings.resolve(R.string.remote_err_forbidden, "这个操作不被允许。")
        else -> strings.resolve(R.string.remote_err_generic, "出错了，请重试。")
    }

    private fun originLabel(origin: RemotePromptOriginFfi, deviceName: String?, turnId: ULong): String = when (origin) {
        RemotePromptOriginFfi.DESKTOP -> strings.resolve(R.string.remote_origin_desktop, "来自桌面")
        RemotePromptOriginFfi.SCHEDULED -> botNameOf(turnId)?.let {
            strings.resolve(R.string.remote_origin_bot_fmt, "来自 Bot「%1\$s」", it)
        } ?: strings.resolve(R.string.remote_origin_scheduled, "定时任务")
        RemotePromptOriginFfi.DEVICE -> when {
            deviceName.isNullOrBlank() -> strings.resolve(R.string.remote_origin_other_device, "来自其他设备")
            deviceName == thisDeviceName -> strings.resolve(R.string.remote_origin_this_device, "来自此设备")
            else -> strings.resolve(R.string.remote_origin_device_fmt, "来自 %1\$s", deviceName)
        }
    }
}

/**
 * Events the meta fields speak for: questions and permissions are the complete
 * pending set of every update, and the model state is folded from controls.
 */
private fun ClientEvent.isHandledByMeta(): Boolean = when (this) {
    is ClientEvent.AskUserQuestion,
    is ClientEvent.AskUserQuestionResolved,
    is ClientEvent.ModelList,
    is ClientEvent.ModelChanged,
    is ClientEvent.ConversationControlsChanged,
    -> true
    else -> false
}
