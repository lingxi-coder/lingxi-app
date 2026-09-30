package com.lingxi.code.conversation

import com.lingxi.code.computerUseSetupStatus
import com.lingxi.code.shouldReshowComputerUseSetup
import com.lingxi.code.shouldShowComputerUseSetup
import com.lingxi.code.computeruse.ComputerUseGrant
import com.lingxi.code.computeruse.ComputerUseSessionState
import com.lingxi.code.computeruse.ComputerUseTier
import com.lingxi.code.computeruse.ComputerUseUiState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerUseSetupStatusTest {
    @Test
    fun reportsOnlyMissingSetupSteps() {
        val status = ComputerUseSetupStatus(
            accessibilityEnabled = true,
            browserAuthorized = false,
            sessionActive = false,
        )

        assertFalse(status.ready)
        assertEquals("授权浏览器、启动控制会话", status.missingSteps())
    }

    @Test
    fun allRequirementsReadyHidesSetupPrompt() {
        val status = ComputerUseSetupStatus(
            accessibilityEnabled = true,
            browserAuthorized = true,
            sessionActive = true,
        )

        assertTrue(status.ready)
        assertEquals("", status.missingSteps())
    }

    @Test
    fun startingSessionWithBrowserGrantIsAlreadyReady() {
        val status = computerUseSetupStatus(
            state = ComputerUseUiState(
                serviceEnabled = true,
                sessionState = ComputerUseSessionState.Starting,
                grants = listOf(
                    ComputerUseGrant(
                        packageName = "com.android.browser",
                        label = "浏览器",
                        tier = ComputerUseTier.Full,
                    ),
                ),
            ),
            browserPackages = setOf("com.android.browser"),
        )

        assertTrue(status.ready)
    }

    @Test
    fun savedBrowserSelectionOnlyLeavesSessionStartMissing() {
        val status = computerUseSetupStatus(
            state = ComputerUseUiState(serviceEnabled = true),
            configuredPackages = setOf("com.android.browser"),
            browserPackages = setOf("com.android.browser"),
        )

        assertFalse(status.ready)
        assertEquals("启动控制会话", status.missingSteps())
    }

    @Test
    fun nonBrowserSelectionStillRequiresBrowserAuthorization() {
        val status = computerUseSetupStatus(
            state = ComputerUseUiState(serviceEnabled = true),
            configuredPackages = setOf("com.miui.gallery"),
            browserPackages = emptySet(),
        )

        assertFalse(status.ready)
        assertEquals("授权浏览器、启动控制会话", status.missingSteps())
    }

    @Test
    fun androidUseToolActivityCreatesAStableReshowKey() {
        val viewModel = ChatViewModel()

        viewModel.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 android_use…",
                id = "tool-7",
                tool = "android_use",
            ),
        )

        assertEquals("0:tool-7", viewModel.state.value.computerUseRequestKey)
    }

    @Test
    fun unrelatedToolDoesNotReplaceTheComputerUseReshowKey() {
        val viewModel = ChatViewModel()
        viewModel.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 android_use…",
                id = "tool-7",
                tool = "android_use",
            ),
        )

        viewModel.reduce(
            ReplyEvent.ToolActivity(
                label = "调用工具 Read…",
                id = "tool-8",
                tool = "Read",
            ),
        )

        assertEquals("0:tool-7", viewModel.state.value.computerUseRequestKey)
    }

    @Test
    fun dismissalStaysHiddenUntilANewAndroidUseRequestNeedsSetup() {
        val unavailable = ComputerUseSetupStatus(
            accessibilityEnabled = false,
            browserAuthorized = false,
            sessionActive = false,
        )

        assertFalse(shouldShowComputerUseSetup(unavailable, dismissed = true))
        assertTrue(
            shouldReshowComputerUseSetup(
                readiness = unavailable,
                requestKey = "42:tool-8",
                handledRequestKey = "42:tool-7",
            ),
        )
        assertFalse(
            shouldReshowComputerUseSetup(
                readiness = unavailable,
                requestKey = "42:tool-7",
                handledRequestKey = "42:tool-7",
            ),
        )
    }

    @Test
    fun configuredComputerUseNeverShowsOrReshowsSetup() {
        val ready = ComputerUseSetupStatus(
            accessibilityEnabled = true,
            browserAuthorized = true,
            sessionActive = true,
        )

        assertFalse(shouldShowComputerUseSetup(ready, dismissed = false))
        assertFalse(
            shouldReshowComputerUseSetup(
                readiness = ready,
                requestKey = "42:tool-8",
                handledRequestKey = null,
            ),
        )
    }
}
