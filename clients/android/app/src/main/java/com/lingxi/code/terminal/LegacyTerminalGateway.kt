/*
 * Copyright (C) 2025 OpenMinis contributors
 * Copyright (C) 2026 LingXi contributors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License, version 3.
 */
package com.lingxi.code.terminal

import android.content.Context
import com.openminis.app.sandbox.PtyBridge
import java.io.File
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.coroutines.yield

/**
 * Interactive shell for the Legacy Android runtime.
 *
 * This follows OpenMinis' TerminalSession design: fork the existing bundled
 * shell into a real bionic PTY, stream raw bytes to the emulator, and keep the
 * terminal independent from the Mobile Linux runtime lifecycle.
 */
class LegacyTerminalGateway internal constructor(
    private val runtime: LegacyPtyRuntime,
) : TerminalSessionGateway {
    constructor(context: Context) : this(AndroidLegacyPtyRuntime(context.applicationContext))

    private val outputFlow = MutableSharedFlow<ByteArray>(
        extraBufferCapacity = 128,
        onBufferOverflow = BufferOverflow.DROP_OLDEST,
    )
    private val stateFlow = MutableStateFlow(TerminalConnectionState.IDLE)
    private val errorFlow = MutableStateFlow<String?>(null)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val lifecycleMutex = Mutex()
    private var session: LegacyPtySession? = null
    private var readerJob: Job? = null
    private var size = LegacyPtySize(DEFAULT_COLUMNS, DEFAULT_ROWS)

    override val output: Flow<ByteArray> = outputFlow.asSharedFlow()
    override val state: StateFlow<TerminalConnectionState> = stateFlow.asStateFlow()
    override val error: StateFlow<String?> = errorFlow.asStateFlow()

    override suspend fun start(sessionId: String): TerminalStartResult =
        lifecycleMutex.withLock {
            if (session != null) return@withLock TerminalStartResult(created = false)
            stateFlow.value = TerminalConnectionState.CONNECTING
            errorFlow.value = null
            try {
                val opened = withContext(Dispatchers.IO) { runtime.open(size) }
                session = opened
                stateFlow.value = TerminalConnectionState.CONNECTED
                readerJob = scope.launch { readLoop(opened) }
                scope.launch { waitForExit(opened) }
                TerminalStartResult(created = true)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                fail(failure)
                throw failure
            }
        }

    override suspend fun write(bytes: ByteArray) {
        if (bytes.isEmpty()) return
        val opened = lifecycleMutex.withLock { session }
            ?: return
        try {
            withContext(Dispatchers.IO) {
                var offset = 0
                while (offset < bytes.size) {
                    val count = minOf(WRITE_CHUNK_SIZE, bytes.size - offset)
                    val written = runtime.write(opened.fd, bytes, offset, count)
                    check(written > 0) {
                        if (written < 0) {
                            "PTY write failed: errno=${-written}"
                        } else {
                            "PTY write returned zero bytes"
                        }
                    }
                    offset += written
                    if (offset < bytes.size) yield()
                }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Throwable) {
            fail(failure)
            throw failure
        }
    }

    override suspend fun resize(columns: Int, rows: Int) {
        if (columns <= 0 || rows <= 0) return
        val next = LegacyPtySize(columns, rows)
        val opened = lifecycleMutex.withLock {
            size = next
            session
        } ?: return
        withContext(Dispatchers.IO) {
            val result = runtime.resize(opened.fd, next)
            check(result >= 0) { "PTY resize failed: errno=${-result}" }
        }
    }

    override suspend fun clear() = Unit

    override suspend fun close() {
        val opened = lifecycleMutex.withLock {
            val current = session
            session = null
            readerJob?.cancel()
            readerJob = null
            stateFlow.value = TerminalConnectionState.CLOSED
            errorFlow.value = null
            current
        } ?: return
        withContext(Dispatchers.IO) {
            runtime.close(opened.fd)
            runtime.signal(opened.pid, SIGTERM)
        }
    }

    private suspend fun readLoop(opened: LegacyPtySession) {
        val buffer = ByteArray(READ_BUFFER_SIZE)
        while (true) {
            val count = runtime.read(opened.fd, buffer, 0, buffer.size)
            if (count <= 0) return
            outputFlow.emit(buffer.copyOf(count))
        }
    }

    private suspend fun waitForExit(opened: LegacyPtySession) {
        val exitStatus = runtime.waitFor(opened.pid)
        val shouldReport = lifecycleMutex.withLock {
            if (session != opened) {
                false
            } else {
                session = null
                readerJob?.cancel()
                readerJob = null
                stateFlow.value = TerminalConnectionState.CLOSED
                true
            }
        }
        if (shouldReport) {
            runtime.close(opened.fd)
            outputFlow.emit("\r\n[Process exited: $exitStatus]\r\n".toByteArray())
        }
    }

    private fun fail(failure: Throwable) {
        val message = failure.message
            ?.takeIf(String::isNotBlank)
            ?: failure::class.java.simpleName
        errorFlow.value = message
        stateFlow.value = TerminalConnectionState.FAILED
    }

    internal interface LegacyPtyRuntime {
        fun open(size: LegacyPtySize): LegacyPtySession
        fun read(fd: Int, buffer: ByteArray, offset: Int, length: Int): Int
        fun write(fd: Int, buffer: ByteArray, offset: Int, length: Int): Int
        fun resize(fd: Int, size: LegacyPtySize): Int
        fun close(fd: Int): Int
        fun signal(pid: Int, signal: Int): Int
        fun waitFor(pid: Int): Int
    }

    private class AndroidLegacyPtyRuntime(
        private val context: Context,
    ) : LegacyPtyRuntime {
        override fun open(size: LegacyPtySize): LegacyPtySession {
            val workspace = File(context.filesDir, "shell/workspaces/default").also {
                check(it.exists() || it.mkdirs()) {
                    "Unable to create terminal workspace at ${it.absolutePath}"
                }
            }
            val appletDirectory = File(context.filesDir, "applet-bin")
            val bundledShell = File(context.applicationInfo.nativeLibraryDir, "libmksh.so")
            val executable = bundledShell.takeIf(File::isFile) ?: File("/system/bin/sh")
            check(executable.isFile) {
                "Interactive shell is missing at ${executable.absolutePath}"
            }
            val argv = arrayOf(
                if (executable == bundledShell) "mksh" else "sh",
                "-l",
                "-i",
            )
            val environment = linkedMapOf(
                "HOME" to workspace.absolutePath,
                "PWD" to workspace.absolutePath,
                "TMPDIR" to context.cacheDir.absolutePath,
                "PATH" to "${appletDirectory.absolutePath}:/system/bin",
                "SHELL" to executable.absolutePath,
                "TERM" to "xterm-256color",
                "LANG" to "C.UTF-8",
                "LC_ALL" to "C.UTF-8",
                "ANDROID_ROOT" to "/system",
                "ANDROID_DATA" to "/data",
            ).map { (key, value) -> "$key=$value" }.toTypedArray()
            val pid = IntArray(1)
            val fd = PtyBridge.forkExec(
                executable.absolutePath,
                argv,
                environment,
                workspace.absolutePath,
                size.columns,
                size.rows,
                pid,
            )
            check(fd >= 0) { "Failed to start PTY: errno=${-fd}" }
            return LegacyPtySession(fd = fd, pid = pid[0])
        }

        override fun read(fd: Int, buffer: ByteArray, offset: Int, length: Int): Int =
            PtyBridge.readBytes(fd, buffer, offset, length)

        override fun write(fd: Int, buffer: ByteArray, offset: Int, length: Int): Int =
            PtyBridge.writeBytes(fd, buffer, offset, length)

        override fun resize(fd: Int, size: LegacyPtySize): Int =
            PtyBridge.setWindowSize(fd, size.columns, size.rows)

        override fun close(fd: Int): Int = PtyBridge.closeFd(fd)

        override fun signal(pid: Int, signal: Int): Int = PtyBridge.sendSignal(pid, signal)

        override fun waitFor(pid: Int): Int = PtyBridge.waitFor(pid)
    }

    internal data class LegacyPtySession(val fd: Int, val pid: Int)
    internal data class LegacyPtySize(val columns: Int, val rows: Int)

    private companion object {
        const val DEFAULT_COLUMNS = 80
        const val DEFAULT_ROWS = 24
        const val READ_BUFFER_SIZE = 8192
        const val WRITE_CHUNK_SIZE = 2048
        const val SIGTERM = 15
    }
}
