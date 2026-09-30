package com.lingxi.code

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredWidth
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.unit.dp
import com.lingxi.code.theme.LingXiTheme
import org.junit.Rule
import org.junit.Test

class AdaptiveConversationDrawerTest {
    @get:Rule val rule = createComposeRule()

    @Test fun crossingTabletBreakpointPreservesConversationState() {
        var width by mutableStateOf(390.dp)
        rule.setContent {
            LingXiTheme(darkTheme = false) {
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
        rule.onNodeWithText("Draft 0").performClick()
        rule.onNodeWithText("Draft 1").assertExists()
        rule.runOnIdle { width = 900.dp }
        rule.onNodeWithText("Projects").assertExists()
        rule.onNodeWithText("Draft 1").assertExists()
        rule.runOnIdle { width = 390.dp }
        rule.onNodeWithText("Draft 1").assertExists()
    }
}
