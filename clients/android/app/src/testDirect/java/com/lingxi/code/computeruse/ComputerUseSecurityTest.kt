package com.lingxi.code.computeruse

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ComputerUseSecurityTest {
    @Test
    fun securityAndLingXiPackagesAreAlwaysBlocked() {
        assertTrue(ComputerUseSecurity.isHardBlockedPackage("com.lingxi.code.direct.debug"))
        assertTrue(
            ComputerUseSecurity.isHardBlockedPackage(
                "com.google.android.permissioncontroller",
            ),
        )
        assertFalse(ComputerUseSecurity.isHardBlockedPackage("com.android.settings"))
    }

    @Test
    fun sendingAndDeletingRequireOneShotConfirmation() {
        assertEquals(
            ComputerUseRisk.ConfirmEveryTime,
            ComputerUseSecurity.riskFor("tap", "发送消息"),
        )
        assertEquals(
            ComputerUseRisk.ConfirmEveryTime,
            ComputerUseSecurity.riskFor("tap", "Delete item"),
        )
        assertEquals(
            ComputerUseRisk.Normal,
            ComputerUseSecurity.riskFor("set_text", "Compose message"),
        )
    }

    @Test
    fun unlabeledCommitActionsRequireOneShotConfirmation() {
        assertEquals(
            ComputerUseRisk.ConfirmEveryTime,
            ComputerUseSecurity.riskFor("tap", "", semanticsReliable = false),
        )
        assertEquals(
            ComputerUseRisk.ConfirmEveryTime,
            ComputerUseSecurity.riskFor("enter", "", semanticsReliable = false),
        )
        assertEquals(
            ComputerUseRisk.Normal,
            ComputerUseSecurity.riskFor("scroll", "", semanticsReliable = false),
        )
    }

    @Test
    fun sensitiveNodeOrIncompleteSecurityScanBlocksWholeSurface() {
        assertTrue(ComputerUseSecurity.isSensitiveIdentity("com.example:id/pin_input"))
        assertFalse(ComputerUseSecurity.isSensitiveIdentity("com.example:id/spinner"))
        assertTrue(
            ComputerUseSecurity.isHardBlockedSurface(
                packageName = "com.example.notes",
                visibleText = "",
                containsSensitiveNode = true,
            ),
        )
        assertTrue(
            ComputerUseSecurity.isHardBlockedSurface(
                packageName = "com.example.notes",
                visibleText = "",
                scanTruncated = true,
            ),
        )
    }

    @Test
    fun backgroundCallsRequireAnActiveSessionAndInMemoryGrants() {
        assertTrue(
            ComputerUseSecurity.hasActiveAuthorization(
                ComputerUseSessionState.Active,
                hasGrants = true,
            ),
        )
        assertTrue(
            ComputerUseSecurity.hasActiveAuthorization(
                ComputerUseSessionState.AwaitingApproval,
                hasGrants = true,
            ),
        )
        assertFalse(
            ComputerUseSecurity.hasActiveAuthorization(
                ComputerUseSessionState.Active,
                hasGrants = false,
            ),
        )
        assertFalse(
            ComputerUseSecurity.hasActiveAuthorization(
                ComputerUseSessionState.Starting,
                hasGrants = true,
            ),
        )
        assertFalse(
            ComputerUseSecurity.hasActiveAuthorization(
                ComputerUseSessionState.Inactive,
                hasGrants = true,
            ),
        )
    }

    @Test
    fun configuredInputMethodIsATransientWindowNotAnAuthorizedApp() {
        assertTrue(
            ComputerUseSecurity.isConfiguredInputMethodPackage(
                packageName = "com.google.android.inputmethod.latin",
                configuredInputMethod =
                    "com.google.android.inputmethod.latin/com.android.inputmethod.latin.LatinIME",
            ),
        )
        assertFalse(
            ComputerUseSecurity.isConfiguredInputMethodPackage(
                packageName = "com.example.notes",
                configuredInputMethod =
                    "com.google.android.inputmethod.latin/com.android.inputmethod.latin.LatinIME",
            ),
        )
        assertFalse(
            ComputerUseSecurity.isConfiguredInputMethodPackage(
                packageName = "com.google.android.inputmethod.latin",
                configuredInputMethod = null,
            ),
        )
    }

    @Test
    fun actionTiersMatchReadClickFullContract() {
        assertEquals(ComputerUseTier.Read, ComputerUseSecurity.requiredTier("screenshot"))
        assertEquals(ComputerUseTier.Click, ComputerUseSecurity.requiredTier("swipe"))
        assertEquals(ComputerUseTier.Full, ComputerUseSecurity.requiredTier("set_text"))
    }
}
