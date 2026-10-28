package com.lingxi.code.runtimeintegration

import com.lingxi.code.bindings.android.AndroidLaunchModeFfi
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.bindings.client.PermissionKindDto
import com.lingxi.code.bindings.client.PermissionRequest
import com.lingxi.code.bindings.client.WorkflowProgressDto
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeEngineLaunchScenarioTest {
    private fun permission() = PermissionRequest(
        requestId = 17uL,
        kind = PermissionKindDto.ToolUseConfirm(
            toolName = "Bash",
            toolInputJson = "{}",
            defaultAllow = false,
        ),
        worker = null,
        owner = null,
        suppressAlwaysAllowRule = false,
        autoModePrompt = null,
    )

    private fun progress() = WorkflowProgressDto(
        kind = "workflow_agent", index = 0u, title = "Worker", message = "working",
        label = null, phaseIndex = 0u, phaseTitle = "Test", agentId = "agent",
        agentType = "test", model = null, fallbackModel = null, state = "running",
        error = null, toolUseId = null, queuedAtMs = 1u, startedAtMs = 2u,
        lastProgressAtMs = 3u, attempt = 1u, lastAttemptReason = null, tokens = null,
        toolCalls = null, lastToolName = null, lastToolSummary = null, promptPreview = null,
    )

    @Test
    fun interactivePreservesTheOwnedListenerAndPermissionSink() = runBlocking {
        val event = ClientEvent.SessionEnded
        val permission = permission()
        val progress = progress()
        var receivedEvent: ClientEvent? = null
        var receivedPermission: PermissionRequest? = null
        var receivedProgress: WorkflowProgressDto? = null
        var receivedIds: List<String>? = null
        val callbacks = NativeEngineCallbacks(
            onEvent = { receivedEvent = it },
            onWorkflowProgress = { origin, task, run, value ->
                receivedIds = listOf(origin, task, run)
                receivedProgress = value
            },
            onPermission = { receivedPermission = it },
        )
        val policy = nativeEngineLaunchPolicy(NativeEngineScenario.Interactive(callbacks))

        assertEquals(AndroidLaunchModeFfi.INTERACTIVE, policy.launchMode)
        assertFalse(policy.appAgentHost)
        assertSame(callbacks, policy.callbacks)
        val listener = policy.callbacks.eventListener()
        listener.onEvent(event)
        listener.onWorkflowProgress("session", "task", "run", progress)
        policy.callbacks.permissionSink().onRequest(permission)
        assertSame(event, receivedEvent)
        assertEquals(listOf("session", "task", "run"), receivedIds)
        assertSame(progress, receivedProgress)
        assertSame(permission, receivedPermission)
    }

    @Test
    fun scheduledAndLocalAppLaunchesKeepTheirDistinctPermissionPolicies() = runBlocking {
        val scheduled = nativeEngineLaunchPolicy(NativeEngineScenario.ScheduledHeadless)
        assertEquals(AndroidLaunchModeFfi.SCHEDULED_HEADLESS, scheduled.launchMode)
        assertFalse(scheduled.appAgentHost)

        val localApp = nativeEngineLaunchPolicy(NativeEngineScenario.LocalAppAgent)
        assertEquals(AndroidLaunchModeFfi.INTERACTIVE, localApp.launchMode)
        assertTrue(localApp.appAgentHost)

        // Neither path has an interactive event or permission consumer. These
        // callbacks must be harmless even when the Android Log stub is absent.
        for (policy in listOf(scheduled, localApp)) {
            policy.callbacks.eventListener().onEvent(ClientEvent.SessionEnded)
            policy.callbacks.permissionSink().onRequest(permission())
        }
    }
}
