package com.lingxi.code.runtime

import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Uses the public terminal route in the complete, installed application. */
@RunWith(AndroidJUnit4::class)
class MobileLinuxProductUiAcceptanceTest {
    @Test fun realTerminalRouteDisplaysDefaultUser() = runBlocking<Unit> {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val directories = com.lingxi.code.project.MobileUserStorage.directories(context)
        val config = LinuxRuntimeBridge.configForWorkspace(context, LinuxRuntimeMode.MobileLinux)
        val name = "product-ui-${UUID.randomUUID()}"
        val proof = File(config.managedRoot, "persistent/${directories.linuxUser}/$name")
        val command = "clear; printf 'PRODUCT_UI:%s:%s:%s\\n' \"\$(id -u)\" \"\$HOME\" \"\$PWD\"; printf '%s:%s:%s' \"\$(id -u)\" \"\$HOME\" \"\$PWD\" > \"\$HOME/$name\"\n"
        val url = Uri.parse(com.lingxi.code.navigation.DeepLink.build("terminal/$name", payload = mapOf("command" to command)))
        val intent = Intent(context, MainActivity::class.java).setAction(Intent.ACTION_VIEW).setData(url)
        try {
            ActivityScenario.launch<MainActivity>(intent).use {
                val deadline = System.currentTimeMillis() + 30_000
                var approved = false
                while (System.currentTimeMillis() < deadline && !approved) {
                    val button = instrumentation.uiAutomation.rootInActiveWindow
                        ?.findAccessibilityNodeInfosByText(context.getString(com.lingxi.code.R.string.terminal_run))
                        ?.firstOrNull { it.isClickable && it.isEnabled }
                    approved = button?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true
                    if (!approved) delay(100)
                }
                assertTrue("The terminal must ask for command approval", approved)
                while (System.currentTimeMillis() < deadline &&
                    !runCatching { proof.readText().startsWith("1000:${directories.guestHome}:${directories.configGuestPath}") }.getOrDefault(false)) delay(100)
                assertTrue("Public terminal route must execute on the real PTY", proof.exists())
                assertTrue(proof.readText(), proof.readText().startsWith("1000:${directories.guestHome}:${directories.configGuestPath}"))
                delay(300)
                val screenshot = File(context.getExternalFilesDir("runtime-acceptance"), "real-product-terminal.png")
                screenshot.parentFile!!.mkdirs()
                screenshot.outputStream().use { output ->
                    instrumentation.uiAutomation.takeScreenshot().compress(Bitmap.CompressFormat.PNG, 100, output)
                }
                android.util.Log.i("ProductAcceptance", "PRODUCT_UI_ACCEPTANCE PASS android screenshot=${screenshot.absolutePath}")
            }
        } finally { proof.delete() }
    }
}
