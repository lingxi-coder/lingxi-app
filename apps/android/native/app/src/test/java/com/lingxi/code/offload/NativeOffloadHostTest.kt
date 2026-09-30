package com.lingxi.code.offload

import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeOffloadHostTest {
    private val clipboard = FakeClipboard()
    private val notifications = FakeNotifications()
    private val ports = NativeOffloadPorts(
        clipboard = clipboard,
        notifications = notifications,
        device = NativeDevicePort { linkedMapOf("model" to "test-device", "sdk" to "35") },
    )

    @Test
    fun storeCatalogCoversEveryStoreToolButExcludesPrivilegedDirectTools() {
        val capabilities = NativeOffloadCatalog.createStoreHost(
            ports = ports,
            permissionGate = allowAll(),
        ).capabilities()

        val names = capabilities.map { it.name }.toSet()
        assertEquals(
            setOf(
                "alarm", "browser", "calendar", "clipboard", "config", "contacts",
                "device", "location", "model-use", "notification", "open", "photos",
                "player", "scheduled", "session", "speech", "weather",
            ),
            names,
        )
        assertFalse("accessibility" in names)
        assertFalse("shizuku" in names)
    }

    @Test
    fun defaultGateFailsClosedForSensitiveHandlers() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(ports)

        val result = host.execute(request("clipboard", "get"))

        assertEquals(NativeOffloadResult.EXIT_PERMISSION_DENIED, result.exitCode)
        assertEquals("PERMISSION_GATE_UNAVAILABLE", result.permissionError?.code)
    }

    @Test
    fun clipboardAliasWritesAndReadsThroughExistingPort() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(ports, allowAll())

        val set = host.execute(request("/usr/bin/android-clipboard", "set", "hello", "world"))
        val get = host.execute(request("clipboard", "get"))

        assertTrue(set.succeeded)
        assertEquals("hello world", get.stdoutText())
    }

    @Test
    fun unavailableHandlerNeverReportsSuccess() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(ports)

        val result = host.execute(request("calendar", "list"))

        assertEquals(NativeOffloadResult.EXIT_UNAVAILABLE, result.exitCode)
        assertTrue(result.stderrText().contains("unavailable"))
    }

    @Test
    fun injectedProductionPortIsAdvertisedAndReceivesTheOriginalRequest() = runTest {
        var received: NativeOffloadRequest? = null
        val wiredPorts = ports.copy(
            commands = mapOf(
                "calendar" to NativeCommandPort { request ->
                    received = request
                    NativeOffloadResult.success("real-result\n")
                },
            ),
        )
        val host = NativeOffloadCatalog.createStoreHost(wiredPorts, allowAll())

        val result = host.execute(request("android-calendar", "list"))

        assertTrue(host.capabilities().single { it.name == "calendar" }.implemented)
        assertEquals("calendar", received?.command)
        assertEquals(listOf("list"), received?.arguments)
        assertEquals("real-result\n", result.stdoutText())
    }

    @Test
    fun permissionGateFailureFailsClosed() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(
            ports = ports,
            permissionGate = NativeOffloadPermissionGate { _, _ -> error("gate offline") },
        )

        val result = host.execute(request("clipboard", "get"))

        assertEquals(NativeOffloadResult.EXIT_PERMISSION_DENIED, result.exitCode)
        assertEquals("PERMISSION_GATE_FAILURE", result.permissionError?.code)
        assertEquals(null, clipboard.value)
    }

    @Test
    fun notificationRequiresExplicitShapeAndUsesPort() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(ports, allowAll())

        val result = host.execute(
            request(
                "notification",
                "post",
                "--title",
                "Build",
                "--body",
                "Complete",
                "--tag",
                "build-1",
            ),
        )

        assertTrue(result.succeeded)
        assertEquals(Notification("Build", "Complete", "build-1"), notifications.last)
    }

    @Test
    fun denialIsStructuredAndHandlerIsNotInvoked() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(
            ports = ports,
            permissionGate = NativeOffloadPermissionGate { _, descriptor ->
                NativeOffloadPermissionDecision.Denied(
                    NativeOffloadPermissionError(
                        code = "USER_DENIED",
                        tool = descriptor.name,
                        message = "declined",
                        recoverable = false,
                    ),
                )
            },
        )

        val result = host.execute(request("notification", "post"))

        assertEquals(NativeOffloadResult.EXIT_PERMISSION_DENIED, result.exitCode)
        assertEquals("USER_DENIED", result.permissionError?.code)
        assertNotNull(result.permissionError)
        assertEquals(null, notifications.last)
    }

    @Test
    fun unknownHandlerUsesShellCompatibleNotFoundExit() = runTest {
        val host = NativeOffloadCatalog.createStoreHost(ports, allowAll())

        val result = host.execute(request("does-not-exist"))

        assertEquals(NativeOffloadResult.EXIT_NOT_FOUND, result.exitCode)
    }

    @Test
    fun processRuntimeFailsExplicitlyAfterDetach() = runTest {
        NativeOffloadRuntime.attach(
            NativeOffloadCatalog.createStoreHost(ports, allowAll()),
        )
        assertTrue(NativeOffloadRuntime.attached)
        NativeOffloadRuntime.detach()

        val result = NativeOffloadRuntime.execute(request("device"))

        assertFalse(NativeOffloadRuntime.attached)
        assertEquals(NativeOffloadResult.EXIT_UNAVAILABLE, result.exitCode)
        assertTrue(result.stderrText().contains("not attached"))
    }

    private fun request(command: String, vararg arguments: String) = NativeOffloadRequest(
        command = command,
        arguments = arguments.toList(),
        sessionId = "session-test",
    )

    private fun allowAll() = NativeOffloadPermissionGate { _, _ ->
        NativeOffloadPermissionDecision.Allowed
    }

    private class FakeClipboard : NativeClipboardPort {
        private var text: String? = null
        val value: String?
            get() = text
        override fun getText(): String? = text
        override fun setText(text: String) {
            this.text = text
        }
    }

    private data class Notification(val title: String, val body: String, val tag: String?)

    private class FakeNotifications : NativeNotificationPort {
        var last: Notification? = null
        override fun post(title: String, body: String, tag: String?) {
            last = Notification(title, body, tag)
        }
    }
}
