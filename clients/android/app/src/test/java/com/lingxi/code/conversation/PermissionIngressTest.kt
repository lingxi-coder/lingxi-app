package com.lingxi.code.conversation

import com.lingxi.code.bindings.PermissionKindDto
import com.lingxi.code.bindings.PermissionRequest
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Test

class PermissionIngressTest {
    private fun request(requestId: ULong, command: String) = PermissionRequest(
        requestId = requestId,
        kind = PermissionKindDto.ToolUseConfirm(
            toolName = "Bash",
            toolInputJson = """{"command":"$command"}""",
            defaultAllow = false,
        ),
        worker = null,
        owner = null,
    )

    @Test
    fun backgroundPermissionIsPublishedWithoutAMainTurn() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val original = request(1uL, "pwd")

        ingress.publish(original)

        assertEquals(permissionRequestToPrompt(original), permissions.value)
    }

    @Test
    fun cancellingMainTurnDoesNotClearAChildPermission() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val child = request(3uL, "npm test")

        ingress.publish(child)
        val snapshot = ingress.beginCancellation()
        ingress.endTurn()
        ingress.restoreAfterFailedCancellation(snapshot)

        assertEquals(permissionRequestToPrompt(child), permissions.value)
    }

    @Test
    fun resolvingHeadAdvancesToTheNextWorkerPermission() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val first = request(4uL, "pwd")
        val second = request(5uL, "whoami")

        ingress.publish(first)
        ingress.publish(second)
        assertEquals(permissionRequestToPrompt(first), permissions.value)

        ingress.resolve(first.requestId)

        assertEquals(permissionRequestToPrompt(second), permissions.value)
    }

    @Test
    fun unrelatedResolutionDoesNotClearTheVisiblePermission() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val current = request(6uL, "whoami")

        ingress.publish(current)
        ingress.resolve(99uL)

        assertEquals(permissionRequestToPrompt(current), permissions.value)
    }
}
