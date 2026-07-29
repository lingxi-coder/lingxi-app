package com.lingxi.code.offload

import com.lingxi.code.computeruse.ComputerUseFeatureProvider

/**
 * Full/Direct distribution catalog.
 *
 * The Direct-only classes are compiled from this source set and therefore do
 * not exist in Play artifacts. They intentionally report unavailable until the
 * Computer Use lifecycle adapter is attached; the registry must never claim a
 * privileged operation succeeded when no adapter ran.
 */
object DirectNativeOffloads {
    fun createHost(
        ports: NativeOffloadPorts = AndroidNativeOffloadPorts.create(),
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
    ): NativeOffloadHost = RegistryNativeOffloadHost(
        handlers = NativeOffloadCatalog.storeHandlers(ports) + listOf(
            accessibilityHandler(),
            DirectUnavailableHandler(
                name = "shizuku",
                aliases = setOf("android-shizuku"),
                reason = "Privileged Android bridge is not linked in this build",
            ),
        ),
        permissionGate = permissionGate,
    )

    private fun accessibilityHandler(): NativeOffloadHandler {
        val context = AndroidNativeOffloadPorts.currentContext()
            ?: return DirectUnavailableHandler(
                name = "accessibility",
                aliases = setOf("android-accessibility", "android-a11y-cli"),
                reason = "Android host context is not attached",
            )
        return PortNativeOffloadHandler(
            descriptor = NativeOffloadDescriptor(
                name = "accessibility",
                aliases = setOf("android-accessibility", "android-a11y-cli"),
                summary = "Inspect and control the LingXi Computer Use accessibility session",
                risk = NativeOffloadRisk.Automation,
                availability = NativeOffloadAvailability.DirectOnly,
                implemented = true,
            ),
            port = NativeCommandPort { request ->
                when (request.arguments.firstOrNull()) {
                    null, "status" -> {
                        val state = ComputerUseFeatureProvider.state.value
                        NativeOffloadResult.success(
                            "available=${ComputerUseFeatureProvider.available}\n" +
                                "serviceEnabled=${state.serviceEnabled}\n" +
                                "sessionState=${state.sessionState}\n" +
                                "captureMode=${state.captureMode}\n" +
                                "activePackage=${state.activePackage.orEmpty()}\n",
                        )
                    }
                    "settings" -> {
                        ComputerUseFeatureProvider.openAccessibilitySettings(context)
                        NativeOffloadResult.success("launched=true\n")
                    }
                    "stop" -> {
                        ComputerUseFeatureProvider.stop(context, "native-offload")
                        NativeOffloadResult.success()
                    }
                    else -> NativeOffloadResult.usage(
                        "Usage: accessibility status | accessibility settings | accessibility stop",
                    )
                }
            },
        )
    }
}

private class DirectUnavailableHandler(
    name: String,
    aliases: Set<String>,
    private val reason: String,
) : NativeOffloadHandler {
    override val descriptor = NativeOffloadDescriptor(
        name = name,
        aliases = aliases,
        summary = reason,
        risk = NativeOffloadRisk.PrivilegedSystem,
        availability = NativeOffloadAvailability.DirectOnly,
        implemented = false,
    )

    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult =
        NativeOffloadResult.unavailable(descriptor.name, reason)
}
