package com.lingxi.code.offload

import android.content.Context
import java.util.concurrent.atomic.AtomicReference

/**
 * Process-global seam used by the Android composition root and the Linux
 * runtime callback.
 *
 * It owns no permission decisions or handler state. [attach] builds the
 * flavor-appropriate immutable host, while [detach] makes later invocations
 * fail explicitly instead of retaining an Activity or silently falling back.
 */
object NativeOffloadRuntime : NativeOffloadHost {
    private val activeHost = AtomicReference<NativeOffloadHost?>(null)
    private val activePorts = AtomicReference<NativeOffloadPorts?>(null)

    fun attach(
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
        ports: NativeOffloadPorts = AndroidNativeOffloadPorts.create(),
    ) {
        activePorts.getAndSet(ports)?.close()
        activeHost.set(
            NativeOffloadFlavorFactory.createHost(
                ports = ports,
                permissionGate = permissionGate,
            ),
        )
    }

    fun attach(
        context: Context,
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
        commandPorts: Map<String, NativeCommandPort> = emptyMap(),
    ) {
        val platformPorts = AndroidNativeOffloadPorts.create(context)
        attach(
            permissionGate = permissionGate,
            ports = platformPorts.copy(
                commands = platformPorts.commands + commandPorts,
            ),
        )
    }

    /**
     * Explicit host injection for tests and embedders with their own platform
     * adapter composition. Production should prefer [attach].
     */
    fun attach(host: NativeOffloadHost) {
        activePorts.getAndSet(null)?.close()
        activeHost.set(host)
    }

    fun detach() {
        activeHost.set(null)
        activePorts.getAndSet(null)?.close()
        AndroidNativeOffloadPorts.detach()
    }

    val attached: Boolean
        get() = activeHost.get() != null

    override fun capabilities(): List<NativeOffloadDescriptor> =
        activeHost.get()?.capabilities().orEmpty()

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        val host = activeHost.get()
            ?: return NativeOffloadResult.unavailable(
                request.command,
                "native offload host is not attached",
            )
        return host.execute(request)
    }
}
