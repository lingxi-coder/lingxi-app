package com.lingxi.code.bots

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.components.UiTags
import com.lingxi.code.remote.RemoteControl
import com.lingxi.code.remote.RemoteRoute
import com.lingxi.code.remote.RemoteSessionScreen
import com.lingxi.code.R
import com.lingxi.code.settings.SettingsRoutes
import com.lingxi.code.tabs.LocalTabHost
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.MainTabShell
import com.lingxi.code.tabs.TabShellState
import com.lingxi.code.testing.finishComposeFixture
import com.lingxi.code.theme.LingXiTheme
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Real BotsTab/account flow, without signing out or modifying an existing account. */
@RunWith(AndroidJUnit4::class)
class BotsSignedOutRouteUiTest {
    @get:Rule val rule = createAndroidComposeRule<ComponentActivity>()
    @After fun closeFixture() = finishComposeFixture(rule.activity)

    @Test fun phoneRetainsARemoteLinkBehindTheSignInGate() = checkGate(wide = false)
    @Test fun wideRetainsARemoteLinkWithoutCreatingASessionPane() = checkGate(wide = true)

    @Test fun aRetainedSessionAlsoGatesWhenItsParentStillShowsIt() {
        val context = rule.activity.applicationContext
        assumeTrue(FirebaseAuth.getInstance().currentUser == null && RemoteControl.manager(context).uid.value == null)
        val shell = TabShellState(AppTab.Bots)
        rule.setContent {
            LingXiTheme {
                CompositionLocalProvider(LocalTabHost provides shell) {
                    RemoteSessionScreen(RemoteControl.manager(context), "validation-host", "Host", "validation-session",
                        null, null, false, {})
                }
            }
        }
        rule.onNodeWithText(rule.activity.getString(R.string.remote_signed_out_title)).assertIsDisplayed()
        rule.onNodeWithText(rule.activity.getString(R.string.remote_sign_in)).performClick()
        rule.runOnIdle {
            assertEquals(AppTab.Settings, shell.selected)
            assertEquals(SettingsRoutes.ACCOUNT, shell.settingsJump?.route)
            assertEquals(AppTab.Bots, shell.settingsJump?.origin)
        }
    }

    @Test fun aDisposedBotsStackRestoresOnlyForTheSameAccount() {
        var owner by mutableStateOf("user-a")
        var visible by mutableStateOf(true)
        lateinit var stack: MutableState<List<RemoteRoute>>
        rule.setContent {
            val saved = rememberSaveableStateHolder()
            if (visible) saved.SaveableStateProvider("bots") {
                stack = rememberSaveable(owner, stateSaver = RemoteRoute.ownedStackSaver(owner)) { mutableStateOf(emptyList()) }
                Text(if (stack.value.isEmpty()) "root" else "session")
            }
        }
        rule.runOnIdle { stack.value = listOf(RemoteRoute.Session("host-a", "session-a")) }
        rule.onNodeWithText("session").assertIsDisplayed()
        rule.runOnIdle { visible = false }
        rule.waitForIdle()
        rule.runOnIdle { visible = true }
        rule.onNodeWithText("session").assertIsDisplayed()
        rule.runOnIdle { visible = false }
        rule.waitForIdle()
        rule.runOnIdle { owner = "user-b"; visible = true }
        rule.onNodeWithText("root").assertIsDisplayed()
    }

    private fun checkGate(wide: Boolean) {
        val context = rule.activity.applicationContext
        assumeTrue("Requires a signed-out acceptance app; never signs out the user's account",
            FirebaseAuth.getInstance().currentUser == null && RemoteControl.manager(context).uid.value == null)
        lateinit var shell: TabShellState
        rule.setContent {
            CompositionLocalProvider(LocalDensity provides Density(0.5f, 1f)) {
                LingXiTheme(darkTheme = false) {
                    shell = remember { TabShellState(AppTab.Bots).apply { requestBots("validation-host", "validation-session") } }
                    Box(Modifier.requiredSize(if (wide) 900.dp else 400.dp, 1200.dp)
                        .consumeWindowInsets(WindowInsets.safeDrawing)) {
                        MainTabShell(shell, false, chats = { Text("fixture-chats") }, settings = {})
                    }
                }
            }
        }
        rule.onNodeWithTag(BotsTags.SIGNED_OUT).assertIsDisplayed()
        AppTab.entries.forEach { rule.onNodeWithTag(UiTags.tab(it.key)).assertIsDisplayed() }
        rule.runOnIdle {
            assertEquals(0, shell.depthOf(AppTab.Bots))
            assertNotNull("The route must wait for sign-in", shell.botsRequest)
        }
        rule.onNodeWithTag(UiTags.tab("chats")).performClick()
        rule.onNodeWithText("fixture-chats").assertIsDisplayed()
        rule.onNodeWithTag(UiTags.tab("bots")).performClick()
        rule.onNodeWithTag(BotsTags.SIGNED_OUT).assertIsDisplayed()
        rule.runOnIdle { assertNotNull(shell.botsRequest) }
    }
}
