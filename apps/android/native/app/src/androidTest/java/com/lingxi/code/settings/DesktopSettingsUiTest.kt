package com.lingxi.code.settings

import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.test.platform.app.InstrumentationRegistry
import android.graphics.Bitmap
import java.io.File
import com.lingxi.code.R
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.theme.LingXiTheme
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class DesktopSettingsUiTest {
    @get:Rule val rule = createComposeRule()
    private fun label(id: Int) = InstrumentationRegistry.getInstrumentation().targetContext.getString(id)

    @Test fun pendingChangesRequireSavedDraftsBeforeReconnect() {
        val registry = SettingsDraftRegistry()
        var reconnects = 0
        val saving = mutableStateOf(false)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                androidx.compose.runtime.CompositionLocalProvider(LocalSettingsDraftRegistry provides registry) {
                    SettingsConnectionBanner(SettingsEngineBridge.State(connected = true, requiresReconnect = true, savingSettings = saving.value)) { reconnects++ }
                }
            }
        }
        val apply = rule.onNodeWithText(label(R.string.settings_parity_apply_saved))
        rule.runOnIdle { registry.set("provider", true) }
        apply.assertIsNotEnabled()
        rule.onNodeWithText(label(R.string.settings_parity_draft_reconnect_blocked)).assertIsDisplayed()
        rule.runOnIdle { registry.remove("provider"); saving.value = true }
        apply.assertIsNotEnabled()
        rule.runOnIdle { saving.value = false }
        apply.assertIsEnabled().performClick()
        rule.runOnIdle { assertEquals(1, reconnects) }
    }

    @Test fun captureNavigationLightAndDark() {
        val dark = mutableStateOf(false)
        rule.setContent { LingXiTheme(darkTheme = dark.value) { Column(Modifier.verticalScroll(rememberScrollState())) { DesktopSettingsNavigation(onNavigate = {}) } } }
        for (mode in listOf(false,true)) {
            rule.runOnIdle { dark.value = mode }
            rule.waitForIdle()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val file = File(context.getExternalFilesDir("parity-screenshots"), "android-settings-${if(mode) "dark" else "light"}.png")
            file.outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG,100,it) }
        }
    }
    @Test fun searchFindsProviderRoutingAndNavigates() {
        var destination = ""
        lateinit var focusManager: androidx.compose.ui.focus.FocusManager
        var keyboard: androidx.compose.ui.platform.SoftwareKeyboardController? = null
        rule.setContent { LingXiTheme(darkTheme = false) {
            focusManager = androidx.compose.ui.platform.LocalFocusManager.current
            keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
            Column(Modifier.verticalScroll(rememberScrollState())) { DesktopSettingsNavigation(onNavigate = { destination = it }) }
        } }
        rule.onNodeWithText(label(R.string.settings_parity_search)).performTextInput("routing")
        // Search navigation is independent of the IME; release its viewport before checking the result.
        rule.runOnIdle { focusManager.clearFocus(); keyboard?.hide() }
        val result = rule.onNodeWithText(label(R.string.settings_parity_custom_providers))
        result.performScrollTo()
        try {
            rule.waitUntil(5_000) { result.isDisplayed() }
        } catch (failure: Throwable) {
            rule.onRoot().printToLog("SettingsSearchFailure")
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val file = File(context.getExternalFilesDir("parity-screenshots"), "android-settings-search-failure.png")
            file.outputStream().use { rule.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) }
            throw failure
        }
        result.assertIsDisplayed().performClick()
        assertEquals(SettingsRoutes.CUSTOM_PROVIDERS,destination)
        rule.onNodeWithText(label(R.string.settings_appearance)).assertDoesNotExist()
    }
    @Test fun disconnectedSettingsExplainRequiredConnection() {
        rule.setContent { LingXiTheme(darkTheme = true) { EngineSettingsPage(SettingsRoutes.CUSTOM_PROVIDERS,SettingsEngineBridge()) } }
        rule.onNodeWithText(label(R.string.settings_parity_disconnected)).assertIsDisplayed()
    }
    @Test fun toolSwitchEditsDraftAndCanInherit() {
        val draft = mutableStateOf("false")
        rule.setContent { LingXiTheme(darkTheme = false) { TypedSettingField("alwaysThinkingEnabled",draft.value,{draft.value=it},false,emptyList()) } }
        rule.onNode(isToggleable()).performClick()
        rule.runOnIdle { assertEquals("true",draft.value) }
        rule.onNodeWithText(label(R.string.settings_parity_inherit)).performClick()
        rule.runOnIdle { assertEquals("null",draft.value) }
    }
    @Test fun managedToolSwitchIsDisabled() {
        rule.setContent { LingXiTheme(darkTheme = true) { TypedSettingField("alwaysThinkingEnabled","true",{},true,emptyList()) } }
        rule.onNode(isToggleable()).assertIsNotEnabled()
    }
}
