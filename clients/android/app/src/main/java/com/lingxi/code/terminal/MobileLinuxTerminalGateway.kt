package com.lingxi.code.terminal

import android.content.Context
import com.lingxi.code.bindings.MobileLinuxEventFfi
import com.lingxi.code.bindings.MobileLinuxEventKindFfi
import com.lingxi.code.bindings.MobileLinuxMountSpecFfi
import com.lingxi.code.bindings.MobileLinuxPtyOpenRequestFfi
import com.lingxi.code.bindings.MobileLinuxPtySessionHandleFfi
import com.lingxi.code.bindings.MobileLinuxPtySizeFfi
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/**
 * PTY adapter over the one native MobileLinux runtime owned by UniFFI.
 *
 * Sessions are keyed by [sessionId] + runtime mode so an Activity recreation can
 * reconnect to the same PTY instead of opening a second shell and replaying the
 * init command.
 */
class MobileLinuxTerminalGateway internal constructor(
    private val runtime: TerminalRuntime,
    private val mode: LinuxRuntimeMode,
) : TerminalSessionGateway {
    constructor(context: Context, mode: LinuxRuntimeMode) : this(
        runtime = BridgeTerminalRuntime(context.applicationContext, mode),
        mode = mode,
    )

    private val mutableOutput = MutableSharedFlow<ByteArray>(extraBufferCapacity = 128)
    private val mutableState = MutableStateFlow(TerminalConnectionState.IDLE)
    private val mutableError = MutableStateFlow<String?>(null)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var attachedSession: SharedSession? = null
    private var attachedKey: SessionKey? = null
    private var bridgeJobs: List<Job> = emptyList()

    override val output: Flow<ByteArray> = mutableOutput.asSharedFlow()
    override val state: StateFlow<TerminalConnectionState> = mutableState.asStateFlow()
    override val error: StateFlow<String?> = mutableError.asStateFlow()

    override suspend fun start(sessionId: String): TerminalStartResult {
        val normalizedSessionId = sessionId.ifBlank { DEFAULT_SESSION_ID }
        val key = SessionKey(mode = mode, sessionId = normalizedSessionId)
        attachedSession?.takeIf { attachedKey == key }?.let {
            return TerminalStartResult(created = false)
        }

        val (session, created) = registryMutex.withLock {
            sessionRegistry[key]?.let { return@withLock it to false }
            SharedSession(
                key = key,
                runtime = runtime,
                removeSelf = { removeSession(key) },
            ).also { sessionRegistry[key] = it } to true
        }

        attach(session, key)
        return try {
            val fresh = session.startIfNeeded()
            TerminalStartResult(created = fresh)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            if (created) removeSession(key)
            throw failure
        }
    }

    override suspend fun write(bytes: ByteArray) {
        attachedSession?.write(bytes)
            ?: error("Terminal is not connected")
    }

    override suspend fun resize(columns: Int, rows: Int) {
        attachedSession?.resize(columns, rows)
    }

    override suspend fun clear() = Unit

    override suspend fun close() {
        bridgeJobs.forEach(Job::cancel)
        bridgeJobs = emptyList()
        val key = attachedKey
        val session = attachedSession
        attachedKey = null
        attachedSession = null
        session?.close()
        if (key != null) removeSession(key)
        mutableState.value = TerminalConnectionState.CLOSED
    }

    private fun attach(session: SharedSession, key: SessionKey) {
        bridgeJobs.forEach(Job::cancel)
        attachedSession = session
        attachedKey = key
        mutableState.value = session.state.value
        mutableError.value = session.error.value
        bridgeJobs = listOf(
            scope.launch {
                session.output.collect { mutableOutput.emit(it) }
            },
            scope.launch {
                session.state.collect { mutableState.value = it }
            },
            scope.launch {
                session.error.collect { mutableError.value = it }
            },
        )
    }

    private suspend fun removeSession(key: SessionKey) {
        registryMutex.withLock {
            sessionRegistry.remove(key)
        }
    }

    internal interface TerminalRuntime {
        suspend fun readEvents(
            afterSequence: ULong? = null,
            limit: UInt? = null,
        ): List<MobileLinuxEventFfi>

        suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi): MobileLinuxPtySessionHandleFfi
        suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray)
        suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi)
        suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi)
    }

    private class BridgeTerminalRuntime(
        private val context: Context,
        private val mode: LinuxRuntimeMode,
    ) : TerminalRuntime {
        override suspend fun readEvents(afterSequence: ULong?, limit: UInt?) =
            LinuxRuntimeBridge.readEvents(
                context = context,
                mode = mode,
                afterSequence = afterSequence,
                limit = limit,
            )

        override suspend fun openPty(request: MobileLinuxPtyOpenRequestFfi) =
            LinuxRuntimeBridge.openPty(context, mode, request)

        override suspend fun writePty(handle: MobileLinuxPtySessionHandleFfi, input: ByteArray) =
            LinuxRuntimeBridge.writePty(context, mode, handle, input)

        override suspend fun resizePty(handle: MobileLinuxPtySessionHandleFfi, size: MobileLinuxPtySizeFfi) =
            LinuxRuntimeBridge.resizePty(context, mode, handle, size)

        override suspend fun closePty(handle: MobileLinuxPtySessionHandleFfi) =
            LinuxRuntimeBridge.closePty(context, mode, handle)
    }

    private class SharedSession(
        private val key: SessionKey,
        private val runtime: TerminalRuntime,
        private val removeSelf: suspend () -> Unit,
    ) {
        private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        private val startMutex = Mutex()
        private val mutableOutput = MutableSharedFlow<ByteArray>(extraBufferCapacity = 128)
        private val mutableState = MutableStateFlow(TerminalConnectionState.IDLE)
        private val mutableError = MutableStateFlow<String?>(null)
        private var handle: MobileLinuxPtySessionHandleFfi? = null
        private var pollJob: Job? = null
        private var eventCursor: ULong? = null
        private var size = MobileLinuxPtySizeFfi(cols = 80u, rows = 24u)
        private var closed = false

        val output: Flow<ByteArray> = mutableOutput.asSharedFlow()
        val state: StateFlow<TerminalConnectionState> = mutableState.asStateFlow()
        val error: StateFlow<String?> = mutableError.asStateFlow()

        suspend fun startIfNeeded(): Boolean = startMutex.withLock {
            if (closed) error("Terminal session ${key.sessionId} is closed")
            if (handle != null || mutableState.value == TerminalConnectionState.CONNECTING) {
                return@withLock false
            }
            mutableState.value = TerminalConnectionState.CONNECTING
            mutableError.value = null
            try {
                val priorEvents = runtime.readEvents(limit = 256u)
                eventCursor = priorEvents.maxOfOrNull { it.sequence }
                val opened = runtime.openPty(
                    MobileLinuxPtyOpenRequestFfi(
                        command = "/bin/sh",
                        args = listOf("-l"),
                        cwd = null,
                        env = emptyList(),
                        size = size,
                        mounts = emptyList<MobileLinuxMountSpecFfi>(),
                    ),
                )
                handle = opened
                mutableState.value = TerminalConnectionState.CONNECTED
                pollJob = scope.launch { pollEvents(opened) }
                true
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                mutableError.value = failure.message ?: "MobileLinux terminal unavailable"
                mutableState.value = TerminalConnectionState.FAILED
                closed = true
                removeSelf()
                throw failure
            }
        }

        suspend fun write(bytes: ByteArray) {
            runtime.writePty(requireHandle(), bytes)
        }

        suspend fun resize(columns: Int, rows: Int) {
            size = MobileLinuxPtySizeFfi(
                cols = columns.coerceIn(2, UShort.MAX_VALUE.toInt()).toUShort(),
                rows = rows.coerceIn(1, UShort.MAX_VALUE.toInt()).toUShort(),
            )
            handle?.let { runtime.resizePty(it, size) }
        }

        suspend fun close() = startMutex.withLock {
            if (closed) return@withLock
            closed = true
            pollJob?.cancel()
            pollJob = null
            val active = handle
            handle = null
            if (active != null) {
                runCatching { runtime.closePty(active) }
            }
            mutableState.value = TerminalConnectionState.CLOSED
            scope.cancel()
        }

        private suspend fun pollEvents(opened: MobileLinuxPtySessionHandleFfi) {
            while (scope.isActive && handle?.id == opened.id) {
                val events = runtime.readEvents(
                    afterSequence = eventCursor,
                    limit = 256u,
                ).sortedBy { it.sequence }
                if (events.isEmpty()) {
                    delay(EVENT_POLL_INTERVAL_MS)
                    continue
                }
                for (event in events) {
                    eventCursor = maxOf(eventCursor ?: 0u, event.sequence)
                    if (event.sessionId != opened.id) continue
                    when (event.kind) {
                        MobileLinuxEventKindFfi.PTY_OUTPUT ->
                            event.data?.takeIf(ByteArray::isNotEmpty)?.let { mutableOutput.emit(it) }
                        MobileLinuxEventKindFfi.PTY_CLOSED -> {
                            mutableState.value = TerminalConnectionState.CLOSED
                            closed = true
                            removeSelf()
                            return
                        }
                        MobileLinuxEventKindFfi.RUNTIME_ERROR -> {
                            mutableError.value = event.detail
                                ?: event.text
                                ?: "MobileLinux PTY failed"
                            mutableState.value = TerminalConnectionState.FAILED
                            closed = true
                            removeSelf()
                            return
                        }
                        else -> Unit
                    }
                }
            }
        }

        private fun requireHandle(): MobileLinuxPtySessionHandleFfi =
            checkNotNull(handle) { mutableError.value ?: "Terminal is not connected" }
    }

    private data class SessionKey(
        val mode: LinuxRuntimeMode,
        val sessionId: String,
    )

    internal companion object {
        const val DEFAULT_SESSION_ID = "interactive"
        const val EVENT_POLL_INTERVAL_MS = 40L

        private val registryMutex = Mutex()
        private val sessionRegistry = mutableMapOf<SessionKey, SharedSession>()
        suspend fun resetSharedSessions() {
            registryMutex.withLock {
                sessionRegistry.clear()
            }
        }
    }
}
