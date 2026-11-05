package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredWidth
import androidx.compose.material3.DrawerState
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import com.lingxi.code.tabs.LocalWideLayout
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class AdaptiveConversationDrawerTest {
    @get:Rule val rule = createComposeRule()

    @Test fun crossingTabletBreakpointPreservesConversationState() {
        var width by mutableStateOf(390.dp)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                // The breakpoint is the shell's: it measures the window once and provides the result.
                CompositionLocalProvider(LocalWideLayout provides (width >= 840.dp)) {
                    Box(Modifier.requiredWidth(width)) {
                        AdaptiveConversationDrawer(
                            drawerState = rememberDrawerState(DrawerValue.Closed),
                            scrimColor = Color.Black,
                            drawerContent = { Text("Projects") },
                        ) {
                            var count by remember { mutableIntStateOf(0) }
                            TextButton(onClick = { count++ }) { Text("Draft $count") }
                        }
                    }
                }
            }
        }
        rule.onNodeWithText("Draft 0").performClick()
        rule.onNodeWithText("Draft 1").assertExists()
        rule.runOnIdle { width = 900.dp }
        rule.onNodeWithText("Projects").assertExists()
        rule.onNodeWithText("Draft 1").assertExists()
        rule.runOnIdle { width = 390.dp }
        rule.onNodeWithText("Draft 1").assertExists()
    }

    @Test fun reportsAnOpenPhoneDrawerSoTheShellCanHideItsTabBar() {
        val reports = mutableListOf<Boolean>()
        var wide by mutableStateOf(false)
        lateinit var drawerState: DrawerState
        lateinit var scope: CoroutineScope
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                CompositionLocalProvider(LocalWideLayout provides wide) {
                    drawerState = rememberDrawerState(DrawerValue.Closed)
                    scope = rememberCoroutineScope()
                    AdaptiveConversationDrawer(
                        drawerState = drawerState,
                        scrimColor = Color.Black,
                        onOpenOrOpeningChange = { reports += it },
                        drawerContent = { Text("Projects") },
                    ) { Text("Chat") }
                }
            }
        }
        rule.runOnIdle { assertEquals(false, reports.last()) }

        rule.runOnUiThread { scope.launch { drawerState.open() } }
        rule.waitForIdle()
        rule.runOnIdle { assertEquals(true, reports.last()) }

        rule.runOnUiThread { scope.launch { drawerState.close() } }
        rule.waitForIdle()
        rule.runOnIdle { assertEquals(false, reports.last()) }

        // A persistent sidebar is never an "open drawer": the rail stays visible next to it.
        rule.runOnUiThread { scope.launch { drawerState.open() } }
        rule.waitForIdle()
        rule.runOnIdle { wide = true }
        rule.waitForIdle()
        rule.runOnIdle { assertEquals(false, reports.last()) }
    }
}
