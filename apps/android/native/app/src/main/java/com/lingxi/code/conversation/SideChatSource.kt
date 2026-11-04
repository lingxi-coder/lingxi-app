package com.lingxi.code.conversation

import com.lingxi.code.bindings.client.*
import com.lingxi.code.bindings.runtime.*
import com.lingxi.code.bindings.android.AndroidEventListener
import com.lingxi.code.bindings.android.AndroidPermissionSink
import com.lingxi.code.bindings.android.createAndroidSideChat
import com.lingxi.code.model.EngineModelState
import com.lingxi.code.model.canonicalSessionId
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*

/** Only explicit side-manager admission rejections prove no turn was started. */
internal fun isKnownSideChatRejection(failure: Throwable): Boolean =
    failure is SideChatNotAdmittedException ||
        (failure is ClientException.Rejected && failure.message in setOf(
            "side chat host is closed", "side chat is closing", "side chat is busy",
            "command is unavailable in side chat",
        ))

/** One RAM-only child. Never owns or closes its parent's engine handle. */
internal class SideChatSource(
    private val handle: MobileEngineHandle,
    private val parentSessionId: String,
    private val currentParentSessionId: () -> String?,
    private val parentAdmission: () -> Boolean,
) : ConversationSource, NativePromptReceiptSource {
    override val turnAdmissionReady: Boolean get() = parentAdmission()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val relay = LosslessEventRelay<ClientEvent>(scope)
    val controls = MutableStateFlow<ConversationControlsDto?>(null)
    private val models = MutableStateFlow(EngineModelState())
    private val permissions = MutableStateFlow<PermissionPromptState?>(null)
    private var ingress = PermissionIngress(permissions)
    @Volatile private var ended = false
    @Volatile private var foreground = true
    @Volatile private var backgroundStopPending = false
    var onEnded: (() -> Unit)? = null
    lateinit var sideId: String
        private set
    override val modelState = models.asStateFlow()
    override val pendingPermission = permissions.asStateFlow()
    override val clientEvents: Flow<ClientEvent> = relay.events

    suspend fun create(requestId: String) {
        requireSideChatParentAdmission(parentAdmission())
        val info = createAndroidSideChat(handle, requestId, null, object : AndroidEventListener {
            override suspend fun onEvent(event: ClientEvent) {
                receiveEvent(event)
            }
            override suspend fun onWorkflowProgress(originSessionId: String, taskId: String, runId: String, progress: WorkflowProgressDto) = Unit
        }, object : AndroidPermissionSink {
            override suspend fun onRequest(request: PermissionRequest) {
                synchronized(this@SideChatSource) { if (!ended && foreground) ingress.publish(request) }
            }
        })
        sideId = info.sideId
        runCatching { submitClientCommand(ClientCommand.ListModels) }
        runCatching { submitClientCommand(ClientCommand.GetConversationControls) }
    }
    @Synchronized
    private fun receiveEvent(event: ClientEvent) {
        if (event is ClientEvent.SideChatEnded) {
            close()
            onEnded?.invoke()
            return
        }
        if (ended) return
        models.value = reduceModelEvent(models.value, event)
        if (event is ClientEvent.ConversationControlsChanged) controls.value = event.controls
        if (event is ClientEvent.PermissionRequestResolved) ingress.resolve(event.requestId)
        relay.offer(event)
    }
    @Synchronized
    fun beginBackgroundStop() {
        backgroundStopPending = true
        setForeground(false)
    }
    @Synchronized
    fun setForeground(value: Boolean) {
        foreground = value
        if (!value) {
            permissions.value = null
            ingress = PermissionIngress(permissions)
        }
    }
    override suspend fun submitClientCommand(command: ClientCommand) = submitCommand(command, { true })

    private suspend fun submitCommand(
        command: ClientCommand,
        tryBeginNative: () -> Boolean,
        bindNativeIdentity: (NativePromptIdentity) -> Boolean = { true },
        onNativeResult: (Result<Unit>) -> Unit = {},
    ) {
        requireSideChatParentAdmission(parentAdmission())
        if (ended) throw SideChatNotAdmittedException("Side chat has ended")
        if (command is ClientCommand.SendPrompt || command is ClientCommand.ForceCompact) {
            if (!foreground || backgroundStopPending) throw SideChatNotAdmittedException("Return to the app and wait for the side turn to stop before sending")
        }
        if (command is ClientCommand.SendPrompt) {
            val identity = currentNativePromptIdentity() ?: throw SideChatNotAdmittedException("Side chat parent session changed")
            if (!bindNativeIdentity(identity)) throw SideChatNotAdmittedException("Side chat owner changed before submission")
        }
        if (!tryBeginNative()) throw CancellationException("Turn cancelled before submission")
        if (command is ClientCommand.SendPrompt) {
            submitWithReceipt(submit = { handle.submitSideChat(sideId, command) }, record = onNativeResult)
        } else handle.submitSideChat(sideId, command)
    }

    override suspend fun currentNativePromptIdentity(): NativePromptIdentity? {
        if (!::sideId.isInitialized || currentParentSessionId()?.let(::canonicalSessionId) != canonicalSessionId(parentSessionId)) return null
        return NativePromptIdentity(parentSessionId, sideId).canonical()
    }

    override suspend fun clearNotAdmitted(identity: NativePromptIdentity, turnId: Long): Boolean =
        identity.sideId != null && currentNativePromptIdentity() == identity.canonical()

    internal suspend fun submitQueuedPrompt(command: ClientCommand.SendPrompt, tryBeginNative: () -> Boolean) =
        submitCommand(command, tryBeginNative)
    override fun submit(text: String, images: List<ImageRefDto>): Flow<ReplyEvent> =
        submitPrompt(text, images, null, null, null)

    override fun submit(text: String, images: List<ImageRefDto>, turnId: Long): Flow<ReplyEvent> =
        submitPrompt(text, images, turnId, null, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
    ): Flow<ReplyEvent> = submitPrompt(text, images, turnId, visualizationContext, null)

    override fun submit(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
    ): Flow<ReplyEvent> = submitPrompt(text, images, turnId, visualizationContext, promptMode)

    override fun submitForComposer(
        text: String, images: List<ImageRefDto>, turnId: Long,
        visualizationContext: VisualizationRef?, promptMode: PromptModeDto?,
        onNotAdmitted: () -> Unit,
        tryBeginNative: () -> Boolean,
        bindNativeIdentity: (NativePromptIdentity) -> Boolean,
        onNativeResult: (Result<Unit>) -> Unit,
    ): Flow<ReplyEvent> = submitPrompt(text, images, turnId, visualizationContext, promptMode, onNotAdmitted, tryBeginNative, bindNativeIdentity, onNativeResult)

    private fun submitPrompt(
        text: String,
        images: List<ImageRefDto>,
        turnId: Long?,
        visualizationContext: VisualizationRef?,
        promptMode: PromptModeDto?,
        onNotAdmitted: (() -> Unit)? = null,
        tryBeginNative: () -> Boolean = { true },
        bindNativeIdentity: (NativePromptIdentity) -> Boolean = { true },
        onNativeResult: (Result<Unit>) -> Unit = {},
    ): Flow<ReplyEvent> =
        mapReplyStream(
            relay.events.onSubscription {
                try {
                    submitCommand(sideChatSendPromptCommand(text, images, turnId, visualizationContext, promptMode), tryBeginNative, bindNativeIdentity, onNativeResult)
                } catch (cancelled: CancellationException) {
                    throw cancelled
                } catch (failure: Throwable) {
                    if (isKnownSideChatRejection(failure)) onNotAdmitted?.invoke()
                    emit(
                        ClientEvent.Error(
                            ErrorKindDto.TRANSPORT,
                            (failure.message ?: "Side chat unavailable").toClientTextDto(),
                        ),
                    )
                }
            }.filter { event ->
                event !is ClientEvent.SlashCommandResult || event.turnId == turnId?.toULong()
            },
            DefaultConversationStrings,
            acceptSlashCommandResults = true,
            expectedTurnId = turnId,
        )
    override suspend fun cancel() = cancel(null)

    override suspend fun cancel(turnId: Long?) {
        submitClientCommand(sideChatCancelCommand(turnId))
        backgroundStopPending = false
    }
    override suspend fun setModel(id: String) = submitClientCommand(ClientCommand.SetModel(id.toClientTextDto()))
    override suspend fun approvePermission(requestId: ULong, response: PermissionResponseDto) =
        submitClientCommand(ClientCommand.ApprovePermission(requestId, response))
    override suspend fun denyPermission(requestId: ULong) = submitClientCommand(ClientCommand.DenyPermission(requestId))
    suspend fun end() {
        synchronized(this) { ended = true; permissions.value = null }
        handle.endSideChat(sideId)
        close()
    }
    @Synchronized
    override fun close() {
        ended = true
        permissions.value = null
        relay.close()
        scope.cancel()
    }
}

internal fun sideChatSendPromptCommand(
    text: String,
    images: List<ImageRefDto>,
    turnId: Long?,
    visualizationContext: VisualizationRef?,
    promptMode: PromptModeDto?,
): ClientCommand.SendPrompt = ClientCommand.SendPrompt(
    text = text.toClientTextDto(),
    promptMode = promptMode,
    images = images,
    turnId = turnId?.toULong(),
    visualizationContext = visualizationContext?.toDto(),
)

internal fun sideChatCancelCommand(turnId: Long?): ClientCommand =
    ClientCommand.Cancel(turnId?.toULong())

/** Exact command boundary: /sideways is an ordinary prompt. */
internal fun sideChatInput(text: String): String? {
    val value = text.trim()
    if (value == "/side") return ""
    if (value.startsWith("/side") && value.getOrNull(5)?.isWhitespace() == true)
        return value.substring(5).trim()
    return null
}

internal enum class SideChatSendRejection { Attachment, Nested }

internal fun sideChatSendRejection(text: String, hasImages: Boolean, isSideChat: Boolean): SideChatSendRejection? {
    if (sideChatInput(text) == null) return null
    if (hasImages) return SideChatSendRejection.Attachment
    if (isSideChat) return SideChatSendRejection.Nested
    return null
}
