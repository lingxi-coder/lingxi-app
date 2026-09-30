package com.lingxi.code.cron

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.unit.dp
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.buildAndroidCronStore
import kotlinx.coroutines.runBlocking
import org.junit.Rule
import org.junit.Test
import java.io.File

/** Screenshot/interaction against the actual native-backed task repository. */
class CronScreenUiTest {
    @get:Rule val compose = createComposeRule()

    @Test
    fun phoneTaskCenterAndDetail(): Unit = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val store = buildAndroidCronStore(context.filesDir.path, null)
        val name = "Daily brief · UI verification"
        val task = store.createConfigured("0 9 * * *", "Prepare a daily project briefing.", true,
            CronAutomation.defaults("provider/model").change("name", name).change("status", "paused").json)
        try {
            AndroidCronRepository.get(context).refresh()
            compose.setContent {
                com.lingxi.code.theme.LingXiTheme(darkTheme = false) {
                    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).verticalScroll(rememberScrollState()).padding(20.dp)) {
                        CronScreen()
                    }
                }
            }
            compose.waitUntil(20_000) { compose.onAllNodesWithText(name).fetchSemanticsNodes().isNotEmpty() }
            compose.onNodeWithText("All").assertIsDisplayed()
            compose.onNodeWithText("Create").assertIsDisplayed()
            compose.onRoot().captureToImage().asAndroidBitmap().let { bitmap ->
                File(context.cacheDir, "cron-phone-list.png").outputStream().use {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                }
            }
            compose.onNodeWithText(name).performClick()
            compose.onNodeWithText("Task details").assertIsDisplayed()
            compose.onNodeWithText("Runs in").assertIsDisplayed()
            compose.onRoot().captureToImage().asAndroidBitmap().let { bitmap ->
                File(context.cacheDir, "cron-phone-detail.png").outputStream().use {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                }
            }
            val expression = compose.onNode(hasText("Advanced 5-field cron (minute hour day month weekday)") and hasSetTextAction())
            expression.performScrollTo().performTextClearance()
            expression.performTextInput("invalid")
            compose.onNodeWithText("Save changes").performScrollTo().performClick()
            compose.waitUntil(20_000) {
                compose.onAllNodesWithText("invalid cron expression", substring = true).fetchSemanticsNodes().isNotEmpty()
            }
            compose.onNodeWithText(name).assertExists()
            org.junit.Assert.assertEquals("0 9 * * *", store.list().first { it.id == task.id }.cron)
        } finally {
            store.delete(task.id)
            store.destroy()
            AndroidCronRepository.get(context).refresh()
        }
    }
    @Test
    fun unconfiguredTaskDraftDisplaysControls() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        compose.setContent {
            com.lingxi.code.theme.LingXiTheme(darkTheme = false) {
                Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).verticalScroll(rememberScrollState()).padding(20.dp)) {
                    CronScreen()
                }
            }
        }
        compose.onNodeWithText("All").assertIsDisplayed()
        compose.onNodeWithText("Create").performClick()
        compose.onNodeWithText("New scheduled task").assertIsDisplayed()
        compose.onNodeWithText("Runs in").assertIsDisplayed()
        compose.onRoot().captureToImage().asAndroidBitmap().let { bitmap ->
            File(context.cacheDir, "cron-phone-draft.png").outputStream().use {
                bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
            }
        }
    }

}
