package com.lingxi.code.offload

import kotlinx.coroutines.CancellationException

interface NativeOffloadHost {
    fun capabilities(): List<NativeOffloadDescriptor>

    suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult
}

interface NativeOffloadHandler {
    val descriptor: NativeOffloadDescriptor

    suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult
}

/**
 * Immutable handler registry and the single permission/error boundary for all
 * Android native offloads.
 */
class RegistryNativeOffloadHost(
    handlers: List<NativeOffloadHandler>,
    private val permissionGate: NativeOffloadPermissionGate =
        FailClosedNativeOffloadPermissionGate,
) : NativeOffloadHost {
    private val descriptors = handlers.map(NativeOffloadHandler::descriptor)
        .sortedBy(NativeOffloadDescriptor::name)

    private val handlersByName: Map<String, NativeOffloadHandler> = buildMap {
        handlers.forEach { handler ->
            val names = handler.descriptor.aliases + handler.descriptor.name
            names.forEach { name ->
                val normalized = normalizeCommand(name)
                require(normalized.isNotEmpty()) { "Native offload name must not be blank" }
                require(put(normalized, handler) == null) {
                    "Duplicate native offload name: $normalized"
                }
            }
        }
    }

    override fun capabilities(): List<NativeOffloadDescriptor> = descriptors

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        val command = normalizeCommand(request.command)
        val handler = handlersByName[command]
            ?: return NativeOffloadResult.notFound(request.command)
        val descriptor = handler.descriptor

        // An unavailable adapter cannot perform a protected operation, so report
        // its real capability state without opening a pointless permission UI.
        if (!descriptor.implemented) {
            return handler.handle(request.copy(command = descriptor.name))
        }

        val decision = try {
            permissionGate.authorize(request, descriptor)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            return NativeOffloadResult.permissionDenied(
                NativeOffloadPermissionError(
                    code = "PERMISSION_GATE_FAILURE",
                    tool = descriptor.name,
                    message = error.message ?: "permission gate failed",
                    recoverable = true,
                ),
            )
        }
        when (decision) {
            NativeOffloadPermissionDecision.Allowed -> Unit
            is NativeOffloadPermissionDecision.Denied ->
                return NativeOffloadResult.permissionDenied(decision.error)
        }

        return try {
            handler.handle(request.copy(command = descriptor.name))
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (denied: NativeOffloadPermissionException) {
            NativeOffloadResult.permissionDenied(denied.error)
        } catch (error: Throwable) {
            NativeOffloadResult.internal(
                descriptor.name,
                error.message ?: error::class.simpleName ?: "unknown failure",
            )
        }
    }

    private fun normalizeCommand(command: String): String =
        command.substringAfterLast('/').trim().lowercase()
}

internal class UnavailableNativeOffloadHandler(
    override val descriptor: NativeOffloadDescriptor,
    private val reason: String,
) : NativeOffloadHandler {
    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult =
        NativeOffloadResult.unavailable(descriptor.name, reason)
}

internal class PortNativeOffloadHandler(
    override val descriptor: NativeOffloadDescriptor,
    private val port: NativeCommandPort,
) : NativeOffloadHandler {
    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult = try {
        port.execute(request)
    } catch (denied: NativeOffloadPermissionException) {
        throw denied
    } catch (denied: SecurityException) {
        throw NativeOffloadPermissionException(
            NativeOffloadPermissionError(
                code = "ANDROID_PERMISSION_DENIED",
                tool = descriptor.name,
                message = denied.message ?: "required Android permission is not granted",
                recoverable = true,
            ),
        )
    }
}
