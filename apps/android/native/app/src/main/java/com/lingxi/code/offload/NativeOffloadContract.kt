package com.lingxi.code.offload

/**
 * One invocation originating from the Linux runtime.
 *
 * [arguments] deliberately excludes [command]. Keeping the executable and its
 * arguments separate avoids the argv[0] disagreement that existed in the
 * reference implementation.
 */
data class NativeOffloadRequest(
    val command: String,
    val arguments: List<String> = emptyList(),
    val stdin: ByteArray = byteArrayOf(),
    val sessionId: String,
    val workingDirectory: String? = null,
    val origin: NativeOffloadOrigin = NativeOffloadOrigin.Agent,
    val environment: Map<String, String> = emptyMap(),
)

enum class NativeOffloadOrigin {
    Agent,
    InteractiveTerminal,
    UserAction,
}

enum class NativeOffloadRisk {
    None,
    PersonalData,
    DeviceAction,
    Automation,
    PrivilegedSystem,
}

enum class NativeOffloadAvailability {
    StoreAndDirect,
    DirectOnly,
}

data class NativeOffloadDescriptor(
    val name: String,
    val aliases: Set<String> = emptySet(),
    val summary: String,
    val risk: NativeOffloadRisk,
    val availability: NativeOffloadAvailability = NativeOffloadAvailability.StoreAndDirect,
    val implemented: Boolean,
)

/**
 * Structured permission failure returned on stderr.
 *
 * The host never owns permission persistence. `Allow once` and `Allow always`
 * remain decisions of the existing engine permission gate; this object only
 * carries a denial across the runtime boundary.
 */
data class NativeOffloadPermissionError(
    val code: String,
    val tool: String,
    val message: String,
    val recoverable: Boolean,
)

class NativeOffloadPermissionException(
    val error: NativeOffloadPermissionError,
) : Exception(error.message)

data class NativeOffloadResult(
    val exitCode: Int,
    val stdout: ByteArray = byteArrayOf(),
    val stderr: ByteArray = byteArrayOf(),
    val permissionError: NativeOffloadPermissionError? = null,
) {
    val succeeded: Boolean
        get() = exitCode == EXIT_SUCCESS

    fun stdoutText(): String = stdout.toString(Charsets.UTF_8)

    fun stderrText(): String = stderr.toString(Charsets.UTF_8)

    companion object {
        const val EXIT_SUCCESS = 0
        const val EXIT_FAILURE = 1
        const val EXIT_USAGE = 64
        const val EXIT_UNAVAILABLE = 69
        const val EXIT_INTERNAL = 70
        const val EXIT_PERMISSION_DENIED = 77
        const val EXIT_NOT_FOUND = 127

        fun success(text: String = "") = NativeOffloadResult(
            exitCode = EXIT_SUCCESS,
            stdout = text.toByteArray(),
        )

        fun usage(message: String) = NativeOffloadResult(
            exitCode = EXIT_USAGE,
            stderr = ensureLine(message).toByteArray(),
        )

        fun unavailable(tool: String, reason: String) = NativeOffloadResult(
            exitCode = EXIT_UNAVAILABLE,
            stderr = ensureLine("$tool: unavailable: $reason").toByteArray(),
        )

        fun permissionDenied(error: NativeOffloadPermissionError) = NativeOffloadResult(
            exitCode = EXIT_PERMISSION_DENIED,
            stderr = ensureLine("${error.tool}: ${error.message}").toByteArray(),
            permissionError = error,
        )

        fun notFound(command: String) = NativeOffloadResult(
            exitCode = EXIT_NOT_FOUND,
            stderr = ensureLine("$command: native offload not found").toByteArray(),
        )

        fun internal(tool: String, message: String) = NativeOffloadResult(
            exitCode = EXIT_INTERNAL,
            stderr = ensureLine("$tool: internal error: $message").toByteArray(),
        )

        private fun ensureLine(text: String): String =
            if (text.endsWith('\n')) text else "$text\n"
    }
}

sealed interface NativeOffloadPermissionDecision {
    data object Allowed : NativeOffloadPermissionDecision
    data class Denied(val error: NativeOffloadPermissionError) : NativeOffloadPermissionDecision
}

/**
 * Adapter onto LingXi's existing permission gate.
 *
 * Implementations must be stateless: durable/session `Allow always` state is
 * owned by the engine gate, never duplicated in the Android offload layer.
 */
fun interface NativeOffloadPermissionGate {
    suspend fun authorize(
        request: NativeOffloadRequest,
        descriptor: NativeOffloadDescriptor,
    ): NativeOffloadPermissionDecision
}

object FailClosedNativeOffloadPermissionGate : NativeOffloadPermissionGate {
    override suspend fun authorize(
        request: NativeOffloadRequest,
        descriptor: NativeOffloadDescriptor,
    ): NativeOffloadPermissionDecision {
        if (descriptor.risk == NativeOffloadRisk.None) {
            return NativeOffloadPermissionDecision.Allowed
        }
        return NativeOffloadPermissionDecision.Denied(
            NativeOffloadPermissionError(
                code = "PERMISSION_GATE_UNAVAILABLE",
                tool = descriptor.name,
                message = "no LingXi permission gate is attached",
                recoverable = true,
            ),
        )
    }
}
