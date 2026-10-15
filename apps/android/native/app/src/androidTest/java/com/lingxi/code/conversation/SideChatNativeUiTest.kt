package com.lingxi.code.conversation

import android.content.Context
import android.content.ContextWrapper
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onFirst
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.bindings.client.ClientCommand
import com.lingxi.code.bindings.client.ClientEvent
import com.lingxi.code.theme.LingXiTheme
import com.lingxi.code.voice.buildVoiceEngine
import java.io.File
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Exercises Kotlin -> real JNI child -> callback -> ViewModel -> Compose. */
@RunWith(AndroidJUnit4::class)
class SideChatNativeUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun compactIsAdmittedSettlesAndLeavesTheParentUsable() = runBlocking {
        val target = InstrumentationRegistry.getInstrumentation().targetContext
        val root = File(target.cacheDir, "side-chat-regression-${UUID.randomUUID()}").apply { mkdirs() }
        // Keep native settings/checkpoints isolated from the installed app.
        val context = object : ContextWrapper(target) {
            override fun getApplicationContext(): Context = this
            override fun getFilesDir(): File = root
        }
        val parentEvents = CopyOnWriteArrayList<ClientEvent>()
        val engine = requireNotNull(buildVoiceEngine(
            context = context, apiBase = "https://invalid.example", apiKey = "", model = "",
            providerProfilesJson = "{}", routingJson = """{"mobileEnabledProfiles":[]}""",
            visionDelegationEnabled = false, onEvent = { parentEvents.add(it) },
            onPermission = { error("No permission expected") },
        ))
        val source = SideChatSource(engine)
        val store = ViewModelStore()
        val visible = mutableStateOf(true)
        try {
            source.create(UUID.randomUUID().toString())
            lateinit var vm: ChatViewModel
            InstrumentationRegistry.getInstrumentation().runOnMainSync {
                vm = ChatViewModel(source)
                store.put("side", vm)
            }
            rule.setContent {
                LingXiTheme(darkTheme = false) {
                    if (visible.value) {
                        val state by vm.state.collectAsState()
                        ChatScreen(state = state, onSend = { vm.send(it) }, onNewChat = {},
                            onSelectModel = vm::selectModel, isDark = false,
                            voiceInputSupported = false, voiceConversationSupported = false)
                    }
                }
            }
            rule.runOnIdle { vm.send("/compact") }
            rule.waitUntil(15_000) {
                val status = vm.state.value.compaction?.status
                status == CompactionProgressStatus.Failed || status == CompactionProgressStatus.Completed
            }
            // There is deliberately no provider/history to summarize. This must
            // be an actual compactor failure, not a command-admission rejection.
            val progress = requireNotNull(vm.state.value.compaction)
            assertEquals(CompactionProgressStatus.Failed, progress.status)
            assertFalse(progress.detail.orEmpty().contains("command is unavailable"))
            assertFalse(progress.detail.isNullOrBlank())
            assertFalse(vm.state.value.streaming)
            assertTrue(vm.state.value.messages.any { it.text == "/compact" })
            rule.onAllNodesWithText("/compact").onFirst().assertIsDisplayed()
            assertFalse(parentEvents.any { it is ClientEvent.CompactionCompleted })

            // A settled failure must release the child's slot and permit retry.
            rule.runOnIdle { vm.send("/compact") }
            rule.waitUntil(15_000) {
                vm.state.value.messages.count { it.text == "/compact" } == 2 &&
                    vm.state.value.compaction?.status == CompactionProgressStatus.Failed
            }
            source.cancel()
            source.end()
            parentEvents.clear()
            engine.submit(ClientCommand.ListModels)
            rule.waitUntil(10_000) { parentEvents.any { it is ClientEvent.ModelList } }
        } finally {
            rule.runOnIdle { visible.value = false; store.clear() }
            source.close()
            engine.destroy()
            root.deleteRecursively()
        }
    }
}
