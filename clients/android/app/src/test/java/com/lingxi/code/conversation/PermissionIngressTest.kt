package com.lingxi.code.conversation

import com.lingxi.code.bindings.PermissionKindDto
import com.lingxi.code.bindings.PermissionRequest
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
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
    )

    @Test
    fun failedCancellationRestoresOriginalPromptButRejectsLateCallback() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val original = request(1uL, "pwd")

        ingress.beginTurn()
        ingress.publish(original)
        val snapshot = ingress.beginCancellation()
        ingress.publish(request(2uL, "late"))

        assertNull(permissions.value)
        ingress.restoreAfterFailedCancellation(snapshot)
        assertEquals(permissionRequestToPrompt(original), permissions.value)
    }

    @Test
    fun terminalEventPreventsFailedCancellationFromRestoringPrompt() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)

        ingress.beginTurn()
        ingress.publish(request(3uL, "pwd"))
        val snapshot = ingress.beginCancellation()
        ingress.endTurn()
        ingress.restoreAfterFailedCancellation(snapshot)

        assertNull(permissions.value)
    }

    @Test
    fun oldCancellationCannotMutateNewTurnGeneration() {
        val permissions = MutableStateFlow<PermissionPromptState?>(null)
        val ingress = PermissionIngress(permissions)
        val current = request(5uL, "whoami")

        ingress.beginTurn()
        ingress.publish(request(4uL, "pwd"))
        val oldSnapshot = ingress.beginCancellation()
        ingress.beginTurn()
        ingress.publish(current)
        ingress.restoreAfterFailedCancellation(oldSnapshot)

        assertEquals(permissionRequestToPrompt(current), permissions.value)
    }
}
