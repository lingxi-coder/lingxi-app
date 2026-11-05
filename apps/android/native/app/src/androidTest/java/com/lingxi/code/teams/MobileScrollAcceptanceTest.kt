package com.lingxi.code.teams

import android.os.Debug
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.os.Trace
import android.view.FrameMetrics
import android.view.Window
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.datastore.preferences.core.PreferenceDataStoreFactory
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreSettings
import com.google.firebase.firestore.MemoryCacheSettings
import com.lingxi.code.conversation.ChatScreen
import com.lingxi.code.conversation.ChatState
import com.lingxi.code.model.EngineModelCatalog
import com.lingxi.code.model.Message
import com.lingxi.code.model.Role
import com.lingxi.code.model.SessionRef
import com.lingxi.code.tabs.AppTab
import com.lingxi.code.tabs.MainTabShell
import com.lingxi.code.tabs.TabShellState
import com.lingxi.code.testing.finishComposeFixture
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Real Choreographer/FrameMetrics, without Compose's virtual test clock or any live account. */
@RunWith(AndroidJUnit4::class)
class MobileScrollAcceptanceTest {
    @Test fun largeDirectoryAndMessageWindowWithRealFrameTiming() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val fixtureRoot = File(context.cacheDir, "mobile-scroll-${UUID.randomUUID()}").apply { mkdirs() }
        val sdk = FirebaseApp.initializeApp(context, FirebaseOptions.Builder()
            .setProjectId("demo-mobile-team-bot-perf").setApplicationId("1:123456789:android:0000000000000000")
            .setApiKey("demo-emulator-only-key").build(), "scroll-${UUID.randomUUID()}")
        val auth = FirebaseAuth.getInstance(sdk).apply { useEmulator("127.0.0.1", 1) }
        val db = FirebaseFirestore.getInstance(sdk).apply {
            firestoreSettings = FirebaseFirestoreSettings.Builder().setHost("127.0.0.1:1").setSslEnabled(false)
                .setLocalCacheSettings(MemoryCacheSettings.newBuilder().build()).build()
        }
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        val prefs = TeamsPrefs(PreferenceDataStoreFactory.create(scope = scope) { File(fixtureRoot, "prefs.preferences_pb") })
        val workspace = TeamsWorkspace(context, scope, auth, db, prefs, "http://127.0.0.1:1")
        // Never start the workspace. The isolated Auth instance has no user, all UI data is synthetic,
        // and offline message rendering disables mark-read. No production account or API is used.
        assertTrue(auth.currentUser == null)
        val uid = "perf-user"
        val team = TeamSummary("perf-team", "Performance fixture", 2, "owner", 1)
        val members = listOf(TeamMember(uid, "Me", "owner", 1), TeamMember("other", "Colleague", "member", 1))
        val teams = TeamsState(uid = uid, teams = listOf(team), detail = TeamDetail(team, members, emptyList()), selectedId = team.id, loaded = true)
        val conversations = (1..500).map { index ->
            TeamMessageConversation("c$index", "channel", "Conversation $index", emptyList(), 400, 400, 0,
                lastMessage = TeamLastMessage("m$index", 400, 1_793_606_400_000L + index * 1_000L, "user", "other", null, "Performance preview $index"))
        }
        val messages = (1..400).map { index -> TeamMessage("m$index", index.toLong(), "other", "user",
            "Performance message $index. A realistic paragraph with enough text to wrap on a phone.", index.toLong(), "change$index", false, null, null,
            createdAt = "2026-11-04T20:00:00Z") }
        val cache = TeamMessageCache(uid, team.id, 1, conversations, mapOf("c1" to TeamMessageThread(messages)))
        val messaging = TeamMessagingState(cache = cache, directoryReady = true, readsReady = true, botsLoaded = true, offline = true)
        val hiddenChats = ChatState(SessionRef("perf-chats", "Hidden Chats"), (1..400).map {
            Message(Role.Ai, "Hidden chat message $it. This transcript remains composed while Teams is selected.")
        }, model = EngineModelCatalog.pending)
        val mode = mutableIntStateOf(0)
        val samples = mutableListOf<LongArray>()
        val samplesLock = Any()
        var capture = false
        val thread = HandlerThread("mobile-scroll-frames").apply { start() }
        val listener = Window.OnFrameMetricsAvailableListener { _, metrics, dropped ->
            synchronized(samplesLock) {
                if (capture) samples += longArrayOf(metrics.getMetric(FrameMetrics.TOTAL_DURATION), metrics.getMetric(FrameMetrics.DEADLINE), dropped.toLong())
            }
        }
        val scenario = ActivityScenario.launch(ComponentActivity::class.java)
        lateinit var activity: ComponentActivity
        var hasActivity = false
        val report = JSONObject().put("conversations", 500).put("teamMessages", 400).put("hiddenChatMessages", 400)
            .put("realFrameClock", true).put("liveAccount", false)
        val phases = JSONArray()
        try {
            scenario.onActivity {
                activity = it
                hasActivity = true
                it.window.addOnFrameMetricsAvailableListener(listener, Handler(thread.looper))
                it.setContent {
                    LingXiTheme(darkTheme = false) {
                        key(mode.intValue) {
                            if (mode.intValue == 2) TeamConversationScreen(workspace, uid, team, teams, messaging, "c1", false, {})
                            else MainTabShell(remember { TabShellState(AppTab.Teams) }, false,
                                chats = { if (mode.intValue == 1) ChatScreen(hiddenChats, onSend = {}, onNewChat = {}, onSelectModel = {}, isDark = false) else Text("Baseline Chats") },
                                settings = {}, bots = {}, botsBadge = MutableStateFlow(0), teamsBadge = MutableStateFlow(0),
                                teams = { TeamsListScreen(uid, teams, messaging, emptySet(), emptyMap(), "", rememberLazyListState(), {}, {}, {}, {}, {}, {}, {}) })
                        }
                    }
                }
            }
            fun visibleText(prefix: String): Boolean {
                // Traverse the real accessibility tree. Compose virtual nodes do not implement
                // AccessibilityNodeProvider's optional find-by-text shortcut on every version.
                fun contains(node: android.view.accessibility.AccessibilityNodeInfo, depth: Int): Boolean {
                    if (node.isVisibleToUser && (node.text?.contains(prefix) == true || node.contentDescription?.contains(prefix) == true)) return true
                    if (depth >= 30) return false
                    return (0 until node.childCount).any { child -> node.getChild(child)?.let { contains(it, depth + 1) } == true }
                }
                return instrumentation.uiAutomation.rootInActiveWindow?.let { contains(it, 0) } == true
            }
            fun swipe(up: Boolean) {
                val size = activity.resources.displayMetrics
                val x = (size.widthPixels * .65).toInt()
                val top = (size.heightPixels * .25).toInt()
                val bottom = (size.heightPixels * .8).toInt()
                android.os.ParcelFileDescriptor.AutoCloseInputStream(instrumentation.uiAutomation.executeShellCommand(
                    "cmd input swipe $x ${if (up) bottom else top} $x ${if (up) top else bottom} 350")).use { it.readBytes() }
                SystemClock.sleep(120)
            }
            fun phase(name: String, target: Int, cookie: Int) {
                instrumentation.runOnMainSync { mode.intValue = target }
                val deadline = SystemClock.uptimeMillis() + 10_000
                val prefix = if (target == 2) "Performance message" else "Conversation"
                while (!visibleText(prefix) && SystemClock.uptimeMillis() < deadline) SystemClock.sleep(50)
                assertTrue("$name did not render its dataset", visibleText(prefix))
                SystemClock.sleep(750)
                synchronized(samplesLock) { samples.clear(); capture = true }
                Trace.beginAsyncSection("mobile-port/$name", cookie)
                repeat(6) { swipe(it < 3) }
                Trace.endAsyncSection("mobile-port/$name", cookie)
                val frames = synchronized(samplesLock) { capture = false; samples.toList() }
                val times = frames.map { it[0] / 1_000_000.0 }.sorted()
                val memory = Debug.MemoryInfo().also { Debug.getMemoryInfo(it) }
                val result = JSONObject().put("name", name).put("frames", times.size)
                    .put("pssMiB", memory.totalPss / 1024.0).put("callbackDrops", frames.sumOf { it[2] })
                if (times.isNotEmpty()) result.put("p50Ms", times[times.size / 2]).put("p95Ms", times[((times.size - 1) * .95).toInt()])
                    .put("overDeadlineFrames", frames.count { it[1] > 0 && it[0] > it[1] })
                phases.put(result)
                report.put("phases", phases)
                File(context.getExternalFilesDir("mobile-port-acceptance"), "scroll-performance.json").apply { parentFile?.mkdirs(); writeText(report.toString(2)) }
                assertTrue("No real frames recorded for $name", frames.size >= 3)
            }
            phase("directory-light-a", 0, 1)
            phase("directory-heavy-a", 1, 2)
            phase("directory-heavy-b", 1, 3)
            phase("directory-light-b", 0, 4)
            phase("messages-400", 2, 5)
        } finally {
            if (hasActivity) {
                instrumentation.runOnMainSync { activity.window.removeOnFrameMetricsAvailableListener(listener) }
                finishComposeFixture(activity)
            }
            scenario.close()
            scope.cancel()
            sdk.delete()
            thread.quitSafely()
        }
    }
}
