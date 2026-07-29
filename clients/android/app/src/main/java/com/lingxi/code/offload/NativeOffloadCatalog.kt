package com.lingxi.code.offload

/**
 * Store-safe catalog. Direct-only handlers live in the `direct` source set, so
 * privileged device-control implementations are absent from Play builds.
 */
object NativeOffloadCatalog {
    fun createStoreHost(
        ports: NativeOffloadPorts = AndroidNativeOffloadPorts.create(),
        permissionGate: NativeOffloadPermissionGate =
            FailClosedNativeOffloadPermissionGate,
    ): NativeOffloadHost = RegistryNativeOffloadHost(
        handlers = storeHandlers(ports),
        permissionGate = permissionGate,
    )

    internal fun storeHandlers(ports: NativeOffloadPorts): List<NativeOffloadHandler> = listOf(
        ClipboardOffloadHandler(ports.clipboard),
        DeviceOffloadHandler(ports.device),
        NotificationOffloadHandler(ports.notifications),
        command(ports, "alarm", "Android alarm adapter is not connected", NativeOffloadRisk.DeviceAction),
        command(ports, "calendar", "Android calendar adapter is not connected", NativeOffloadRisk.PersonalData),
        command(ports, "contacts", "Android contacts adapter is not connected", NativeOffloadRisk.PersonalData),
        command(ports, "location", "Android location adapter is not connected", NativeOffloadRisk.PersonalData),
        command(ports, "open", "Android open adapter is not connected", NativeOffloadRisk.DeviceAction),
        command(ports, "photos", "Android MediaStore adapter is not connected", NativeOffloadRisk.PersonalData),
        command(ports, "player", "Android media player adapter is not connected", NativeOffloadRisk.DeviceAction),
        command(ports, "speech", "LingXi speech provider is not attached", NativeOffloadRisk.DeviceAction),
        command(ports, "weather", "No Android weather provider is configured", NativeOffloadRisk.PersonalData),
        command(ports, "config", "LingXi config bridge is not attached", NativeOffloadRisk.DeviceAction),
        command(ports, "browser", "LingXi browser bridge is not attached", NativeOffloadRisk.DeviceAction),
        command(ports, "session", "LingXi session bridge is not attached", NativeOffloadRisk.DeviceAction),
        command(ports, "scheduled", "LingXi scheduled-task bridge is not attached", NativeOffloadRisk.DeviceAction),
        command(ports, "model-use", "LingXi model-use bridge is not attached", NativeOffloadRisk.DeviceAction),
    )

    private fun command(
        ports: NativeOffloadPorts,
        name: String,
        unavailableReason: String,
        risk: NativeOffloadRisk,
    ): NativeOffloadHandler {
        val port = ports.commands[name]
            ?: return unavailable(name, unavailableReason, risk)
        return PortNativeOffloadHandler(
            descriptor = NativeOffloadDescriptor(
                name = name,
                aliases = setOf("android-$name"),
                summary = "Android $name native capability",
                risk = risk,
                implemented = true,
            ),
            port = port,
        )
    }

    internal fun unavailable(
        name: String,
        reason: String,
        risk: NativeOffloadRisk,
        aliases: Set<String> = setOf("android-$name"),
        availability: NativeOffloadAvailability = NativeOffloadAvailability.StoreAndDirect,
    ): NativeOffloadHandler = UnavailableNativeOffloadHandler(
        descriptor = NativeOffloadDescriptor(
            name = name,
            aliases = aliases,
            summary = reason,
            risk = risk,
            availability = availability,
            implemented = false,
        ),
        reason = reason,
    )
}

private class ClipboardOffloadHandler(
    private val clipboard: NativeClipboardPort,
) : NativeOffloadHandler {
    override val descriptor = NativeOffloadDescriptor(
        name = "clipboard",
        aliases = setOf("android-clipboard"),
        summary = "Read or write the Android clipboard",
        risk = NativeOffloadRisk.PersonalData,
        implemented = true,
    )

    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult {
        return when (request.arguments.firstOrNull()) {
            "get" -> NativeOffloadResult.success(clipboard.getText().orEmpty())
            "set" -> {
                val text = request.arguments.drop(1).joinToString(" ")
                    .ifEmpty { request.stdin.toString(Charsets.UTF_8) }
                if (text.isEmpty()) {
                    NativeOffloadResult.usage("clipboard set <text> (or provide stdin)")
                } else {
                    clipboard.setText(text)
                    NativeOffloadResult.success()
                }
            }
            "-h", "--help" -> NativeOffloadResult.success(
                "Usage: clipboard get | clipboard set <text>\n",
            )
            else -> NativeOffloadResult.usage("Usage: clipboard get | clipboard set <text>")
        }
    }
}

private class NotificationOffloadHandler(
    private val notifications: NativeNotificationPort,
) : NativeOffloadHandler {
    override val descriptor = NativeOffloadDescriptor(
        name = "notification",
        aliases = setOf("android-notification", "notify"),
        summary = "Post an Android notification",
        risk = NativeOffloadRisk.DeviceAction,
        implemented = true,
    )

    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult {
        if (request.arguments.firstOrNull() in setOf("-h", "--help")) {
            return NativeOffloadResult.success(
                "Usage: notification post --title <title> --body <body> [--tag <tag>]\n",
            )
        }
        if (request.arguments.firstOrNull() != "post") {
            return NativeOffloadResult.usage(
                "Usage: notification post --title <title> --body <body> [--tag <tag>]",
            )
        }
        val options = parseOptions(request.arguments.drop(1))
            ?: return NativeOffloadResult.usage("notification: malformed option")
        val title = options["title"]
            ?: return NativeOffloadResult.usage("notification: --title is required")
        val body = options["body"]
            ?: return NativeOffloadResult.usage("notification: --body is required")
        notifications.post(title, body, options["tag"])
        return NativeOffloadResult.success()
    }

    private fun parseOptions(arguments: List<String>): Map<String, String>? {
        val values = linkedMapOf<String, String>()
        var index = 0
        while (index < arguments.size) {
            val key = arguments[index]
            if (!key.startsWith("--") || index + 1 >= arguments.size) return null
            val normalized = key.removePrefix("--")
            if (normalized !in setOf("title", "body", "tag")) return null
            values[normalized] = arguments[index + 1]
            index += 2
        }
        return values
    }
}

private class DeviceOffloadHandler(
    private val device: NativeDevicePort,
) : NativeOffloadHandler {
    override val descriptor = NativeOffloadDescriptor(
        name = "device",
        aliases = setOf("android-device"),
        summary = "Report non-sensitive Android device information",
        risk = NativeOffloadRisk.None,
        implemented = true,
    )

    override suspend fun handle(request: NativeOffloadRequest): NativeOffloadResult {
        if (request.arguments.isNotEmpty() && request.arguments != listOf("info")) {
            return NativeOffloadResult.usage("Usage: device [info]")
        }
        val output = device.snapshot().entries.joinToString(
            separator = "\n",
            postfix = "\n",
        ) { (key, value) -> "$key=$value" }
        return NativeOffloadResult.success(output)
    }
}
