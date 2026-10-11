package com.lingxi.code.account

import android.accessibilityservice.AccessibilityService
import android.graphics.Bitmap
import android.graphics.Rect
import android.os.SystemClock
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.theme.LingXiTheme
import java.io.File
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.tasks.await
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/** Opt-in, temporary-account UI smoke. Run register and restore in separate app processes. */
@RunWith(AndroidJUnit4::class)
class FirebaseAccountUiTest {
    @get:Rule val rule = createComposeRule()

    @Test fun liveAuthentication() {
        val args = InstrumentationRegistry.getArguments()
        assumeTrue(args.getString("firebaseLive") == "true")
        val phase = requireNotNull(args.getString("phase"))
        val auth = FirebaseAuth.getInstance()
        val email = args.getString("testEmail") ?: ""
        val password = args.getString("testPassword") ?: ""
        if (phase == "restore") assertEquals("SDK must restore the account before any sign-in", email, auth.currentUser?.email)
        else auth.signOut()
        val model = AccountViewModel()
        rule.setContent {
            LingXiTheme(darkTheme = false) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) { LingxiAccountSection(model) }
            }
        }
        if (phase == "google") {
            rule.onNodeWithTag("account.google").performScrollTo().performClick()
            val ui = InstrumentationRegistry.getInstrumentation().uiAutomation
            rule.waitUntil(60_000) {
                val root = ui.rootInActiveWindow
                if (root?.packageName?.toString() != "com.google.android.gms") false
                else {
                    // Decline the new emulator's optional carrier-number lookup.
                    if (descendants(root).any { it.text?.toString() == "Sign in with ease" }) {
                        descendants(root).firstOrNull { it.text?.toString().equals("Skip", ignoreCase = true) }?.let {
                            click(it)
                        }
                    }
                    descendants(root).any { it.className?.toString() == "android.widget.EditText" }
                }
            }
            ui.waitForIdle(500, 3_000)
            capture("android-google-authorization")
            // The provider may have an account-entry page above its account chooser.
            repeat(6) {
                if (model.state.value.busy) {
                    val root = ui.rootInActiveWindow
                    if (root != null && descendants(root).any { it.text?.toString() == "Skip account setup?" }) {
                        descendants(root).firstOrNull { it.text?.toString() == "Skip" }?.let(::click)
                    } else ui.performGlobalAction(AccessibilityService.GLOBAL_ACTION_BACK)
                    val deadline = System.currentTimeMillis() + 3_000
                    while (model.state.value.busy && System.currentTimeMillis() < deadline) Thread.sleep(100)
                }
            }
            rule.waitUntil(20_000) { !model.state.value.busy }
            assertNull(auth.currentUser)
            assertEquals("登录已取消。", model.state.value.message)
            return
        }
        if (phase == "register") {
            rule.onNodeWithTag("account.mode").performScrollTo().performClick()
            credentials(email, password)
            rule.waitUntil(40_000) { auth.currentUser?.email == email && !model.state.value.busy }
            rule.onNodeWithTag("account.profile").assertTextEquals(email)
            capture("android-firebase-registered")
            rule.onNodeWithTag("account.signout").performScrollTo().performClick()
            rule.waitUntil(20_000) { auth.currentUser == null && !model.state.value.busy }
            rule.onNodeWithTag("account.mode").performScrollTo().performClick()
            credentials(email, "wrong-password-123")
            rule.waitUntil(30_000) { !model.state.value.busy && model.state.value.message != null }
            assertNull(auth.currentUser)
            assertEquals("邮箱或密码不正确。", model.state.value.message)
            rule.onNodeWithTag("account.password").performScrollTo().performTextReplacement(password)
            rule.onNodeWithTag("account.submit").performScrollTo().performClick()
            rule.waitUntil(40_000) { auth.currentUser?.email == email && !model.state.value.busy }
            capture("android-firebase-signed-in")
        } else {
            rule.onNodeWithTag("account.profile").assertTextEquals(email)
            capture("android-firebase-restored")
            runBlocking { requireNotNull(auth.currentUser).delete().await() }
            rule.waitUntil(15_000) { auth.currentUser == null }
        }
    }

    private fun click(node: AccessibilityNodeInfo) {
        val bounds = Rect()
        node.getBoundsInScreen(bounds)
        check(!bounds.isEmpty) { "Provider control must have visible bounds" }
        val ui = InstrumentationRegistry.getInstrumentation().uiAutomation
        val now = SystemClock.uptimeMillis()
        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
            val event = MotionEvent.obtain(now, SystemClock.uptimeMillis(), action, bounds.exactCenterX(), bounds.exactCenterY(), 0)
            try { check(ui.injectInputEvent(event, true)) } finally { event.recycle() }
        }
    }

    private fun descendants(node: AccessibilityNodeInfo): Sequence<AccessibilityNodeInfo> = sequence {
        yield(node)
        for (index in 0 until node.childCount) node.getChild(index)?.let { yieldAll(descendants(it)) }
    }

    private fun credentials(email: String, password: String) {
        rule.onNodeWithTag("account.email").performScrollTo().performTextReplacement(email)
        rule.onNodeWithTag("account.password").performScrollTo().performTextReplacement(password)
        rule.onNodeWithTag("account.submit").performScrollTo().performClick()
    }
    private fun capture(name: String) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val directory = requireNotNull(instrumentation.targetContext.getExternalFilesDir("auth-verification"))
        directory.mkdirs()
        val bitmap = instrumentation.uiAutomation.takeScreenshot()
        File(directory, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        bitmap.recycle()
    }
}
