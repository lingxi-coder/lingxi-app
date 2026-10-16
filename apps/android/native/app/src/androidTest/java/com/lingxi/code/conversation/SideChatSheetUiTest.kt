package com.lingxi.code.conversation

import android.content.Context
import android.content.ContextWrapper
import android.graphics.Bitmap
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class SideChatSheetUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun emptyOpenShowsComposerAndReopeningKeepsIndependentDrafts() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val target = instrumentation.targetContext
        val root = File(target.cacheDir, "side-sheet-${UUID.randomUUID()}").apply { mkdirs() }
        val context = object : ContextWrapper(target) {
            override fun getApplicationContext(): Context = this
            override fun getFilesDir(): File = root
            override fun getSharedPreferences(name: String, mode: Int) = super.getSharedPreferences("side-sheet-test-$name", mode)
        }
        val parent = EngineConversationSource.create(context, reuseProcessSource = false)
        assertTrue(parent is EngineConversationSource)
        val store = ViewModelStore()
        lateinit var vm: ChatViewModel
        instrumentation.runOnMainSync { vm = ChatViewModel(source = parent); store.put("parent", vm) }
        val visible = mutableStateOf(true)
        val parentDraft = mutableStateOf("Parent draft remains")
        try {
            rule.setContent {
                LingXiTheme(darkTheme = false) {
                    if (visible.value) {
                        val state by vm.state.collectAsState()
                        ChatScreen(state, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false,
                            draft = parentDraft.value, onDraftChange = { parentDraft.value = it },
                            onOpenSideChats = vm::openSideChats)
                        SideChatSheet(vm.sideChats, false, onCreate = vm::createSideChat)
                    }
                }
            }
            rule.onNodeWithTag("conversation.side-chats").performClick()
            rule.waitUntil(15_000) { vm.sideChats.chats.value.size == 1 && vm.sideChats.visible.value }
            val first = vm.sideChats.selected.value!!
            val input = rule.onNode(hasSetTextAction() and hasAnyAncestor(hasTestTag("side-chat.sheet")))
            input.assertIsDisplayed().assertIsEnabled().performClick().performTextInput("First side draft")
            rule.runOnIdle { assertEquals("First side draft", first.draft.value) }
            lateinit var activity: android.app.Activity
            instrumentation.runOnMainSync {
                activity = androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry.getInstance()
                    .getActivitiesInStage(androidx.test.runner.lifecycle.Stage.RESUMED).first()
                androidx.core.view.WindowCompat.getInsetsController(activity.window, activity.window.decorView)
                    .show(androidx.core.view.WindowInsetsCompat.Type.ime())
            }
            rule.waitUntil(10_000) {
                activity.window.decorView.rootWindowInsets?.isVisible(android.view.WindowInsets.Type.ime()) == true
            }
            val imeTop = activity.windowManager.currentWindowMetrics.bounds.height() -
                activity.window.decorView.rootWindowInsets.getInsets(android.view.WindowInsets.Type.ime()).bottom
            val send = rule.onNode(hasTestTag(com.lingxi.code.components.UiTags.COMPOSER_SEND) and
                hasAnyAncestor(hasTestTag("side-chat.sheet"))).assertIsDisplayed().fetchSemanticsNode().boundsInRoot
            assertTrue("Side-chat send must remain above the keyboard", send.bottom <= imeTop + 1f)
            val keyboardShot = File(target.getExternalFilesDir("side-chat-ui-audit"), "side-composer-keyboard.png")
            keyboardShot.parentFile?.mkdirs()
            keyboardShot.outputStream().use { instrumentation.uiAutomation.takeScreenshot().compress(Bitmap.CompressFormat.PNG, 100, it) }
            androidx.test.espresso.Espresso.closeSoftKeyboard()
            rule.onNodeWithTag("side-chat.create").performClick()
            rule.waitUntil(15_000) { vm.sideChats.chats.value.size == 2 }
            input.performClick().performTextInput("Second side draft")
            val second = vm.sideChats.selected.value!!
            assertNotEquals(first, second)
            rule.onNodeWithTag("side-chat.select").performClick()
            rule.onNodeWithText(target.getString(R.string.side_chat_number, 1)).performClick()
            input.assertTextContains("First side draft")
            val screenshot = File(target.getExternalFilesDir("side-chat-ui-audit"), "side-composer.png")
            screenshot.parentFile?.mkdirs()
            screenshot.outputStream().use { instrumentation.uiAutomation.takeScreenshot().compress(Bitmap.CompressFormat.PNG, 100, it) }
            rule.onNodeWithTag("side-chat.hide").performClick()
            rule.onNodeWithTag("conversation.side-chats").performClick()
            input.assertIsDisplayed().assertTextContains("First side draft")
            rule.runOnIdle {
                assertEquals(2, vm.sideChats.chats.value.size)
                assertEquals("Second side draft", second.draft.value)
                assertEquals("Parent draft remains", parentDraft.value)
                assertTrue(first.viewModel.state.value.messages.isEmpty())
            }
        } finally {
            rule.runOnIdle { visible.value = false; store.clear() }
            root.deleteRecursively()
        }
    }
}
