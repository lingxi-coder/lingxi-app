package com.lingxi.code.offload

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DirectNativeOffloadHostTest {
    @Test
    fun directCatalogRegistersAllFullDistributionTools() {
        val ports = NativeOffloadPorts(
            clipboard = object : NativeClipboardPort {
                override fun getText(): String? = null
                override fun setText(text: String) = Unit
            },
            notifications = NativeNotificationPort { _, _, _ -> },
            device = NativeDevicePort { emptyMap() },
        )

        val capabilities = DirectNativeOffloads.createHost(
            ports = ports,
            permissionGate = NativeOffloadPermissionGate { _, _ ->
                NativeOffloadPermissionDecision.Allowed
            },
        ).capabilities()

        assertEquals(19, capabilities.size)
        assertTrue(capabilities.any { it.name == "accessibility" && !it.implemented })
        assertTrue(capabilities.any { it.name == "shizuku" && !it.implemented })
        assertTrue(
            capabilities
                .filter { it.name in setOf("accessibility", "shizuku") }
                .all { it.availability == NativeOffloadAvailability.DirectOnly },
        )
    }
}
