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
            chromeAuthorized = false,
            sessionActive = false,
        )

        assertFalse(status.ready)
        assertEquals("授权 Chrome、启动控制会话", status.missingSteps)
    }

    @Test
    fun allRequirementsReadyHidesSetupPrompt() {
        val status = ComputerUseSetupStatus(
            accessibilityEnabled = true,
            chromeAuthorized = true,
            sessionActive = true,
        )

        assertTrue(status.ready)
        assertEquals("", status.missingSteps)
    }

    @Test
    fun startingSessionWithChromeGrantIsAlreadyReady() {
        val status = computerUseSetupStatus(
            ComputerUseUiState(
                serviceEnabled = true,
                sessionState = ComputerUseSessionState.Starting,
                grants = listOf(
                    ComputerUseGrant(
                        packageName = "com.android.chrome",
                        label = "Chrome",
                        tier = ComputerUseTier.Full,
                    ),
                ),
            ),
        )

        assertTrue(status.ready)
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
            chromeAuthorized = false,
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
            chromeAuthorized = true,
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
