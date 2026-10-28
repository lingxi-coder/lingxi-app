package com.lingxi.code.runtime

import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import com.lingxi.code.acceptance.requireAcceptanceOptIn
import com.lingxi.code.bindings.android.*
import com.lingxi.code.project.ProjectRepository
import com.lingxi.code.settings.LinuxRuntimeBridge
import com.lingxi.code.settings.LinuxRuntimeMode
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

/** Exercises the installed product, its bundled assets, product project mapper and real JNI. */
@RunWith(AndroidJUnit4::class)
class MobileLinuxProductAcceptanceTest {
    @Test
    fun defaultUserProjectTerminalAndPersistentHome() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "mobileLinuxProductAcceptance", "The product runtime acceptance is run as a staged check",
            "com.lingxi.code.runtime.MobileLinuxProductAcceptanceTest#defaultUserProjectTerminalAndPersistentHome",
        )
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val directories = com.lingxi.code.project.MobileUserStorage.directories(context)
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            instrumentation.waitForIdleSync()
            scenario.onActivity { assertFalse(it.isFinishing) }
            val mode = LinuxRuntimeMode.MobileLinux
            when (LinuxRuntimeBridge.load(context, mode).rootfsState) {
                MobileLinuxRootfsStateFfi.MISSING -> LinuxRuntimeBridge.install(context, mode)
                MobileLinuxRootfsStateFfi.CORRUPT -> LinuxRuntimeBridge.repair(context, mode)
                else -> Unit
            }
            val fixture = File(context.filesDir, "runtime-acceptance-${UUID.randomUUID()}")
            val project = ProjectRepository(
                projectsRoot = File(fixture, "projects"),
                userHome = File(fixture, "home"),
                projectsGuestRoot = directories.projectsGuestPath,
                userHomeGuestPath = directories.guestHome,
            ).createInternal("Runtime acceptance")
            val config = LinuxRuntimeBridge.configForWorkspace(context, mode, project.workspace)
            val runtime = buildAndroidMobileLinuxRuntimeHandle(config)
            val marker = "product-acceptance-${UUID.randomUUID()}"
            suspend fun command(script: String, cwd: String? = null): String {
                val result = runtime.runCommand(MobileLinuxCommandRequestFfi(
                    command = "/bin/sh", args = listOf("-lc", "set -e; $script"), cwd = cwd,
                    env = emptyList(), stdin = null, timeoutMs = 30_000uL,
                    allowNetwork = false, mounts = emptyList(),
                ))
                assertEquals(result.stderr, 0, result.exitCode)
                return result.stdout.trim()
            }
            try {
                runtime.configureMounts(listOf(MobileLinuxMountSpecFfi(
                    hostPath = File(project.workspace.hostPath).canonicalPath, guestPath = project.workspace.guestPath,
                    readOnly = false, purpose = MobileLinuxMountPurposeFfi.WORKSPACE)))
                val boot = runtime.boot()
                assertEquals(boot.lastError, MobileLinuxRootfsStateFfi.READY, boot.state)
                val identity = command("id -un; id -u; id -g; printf '%s\\n' \"\$HOME\" \"\$USER\"; pwd; sudo -n id -u; sudo -ni sh -c 'printf %s \"\$HOME\"'")
                assertEquals("${directories.linuxUser}\n1000\n1000\n${directories.guestHome}\n${directories.linuxUser}\n${directories.defaultGuestCwd}\n0\n/root", identity)
                command("printf '$marker' > \"\$HOME/$marker\"; printf project-ok > product-proof", project.workspace.guestPath)
                assertEquals("project-ok", File(project.workspace.hostPath, "product-proof").readText())
                runtime.spawnBackground(MobileLinuxCommandRequestFfi(
                    command = "/bin/sh", args = listOf("-lc", "id -u > background-proof"), cwd = project.workspace.guestPath,
                    env = emptyList(), stdin = null, timeoutMs = 10_000uL, allowNetwork = false, mounts = emptyList(),
                ))
                val backgroundProof = File(project.workspace.hostPath, "background-proof")
                val backgroundDeadline = System.currentTimeMillis() + 15_000
                while (System.currentTimeMillis() < backgroundDeadline && runCatching { backgroundProof.readText().trim() }.getOrNull() != "1000") delay(100)
                assertEquals("1000", backgroundProof.readText().trim())
                val pty = runtime.openPty(MobileLinuxPtyOpenRequestFfi(
                    command = "/bin/sh", args = listOf("-i"), cwd = project.workspace.guestPath,
                    env = emptyList(), size = MobileLinuxPtySizeFfi(cols = 80u, rows = 24u), mounts = emptyList(),
                ))
                runtime.writePty(pty, "printf 'PRODUCT_PTY:%s:%s:%s\\n' \"\$(id -u)\" \"\$HOME\" \"\$PWD\"\n".toByteArray())
                val expected = "PRODUCT_PTY:1000:${directories.guestHome}:${project.workspace.guestPath}"
                var output = ""
                var cursor: ULong? = null
                val deadline = System.currentTimeMillis() + 20_000
                while (System.currentTimeMillis() < deadline && !output.contains(expected)) {
                    runtime.readEvents(cursor, 256u).forEach { event ->
                        cursor = event.sequence
                        output += event.text ?: event.data?.decodeToString().orEmpty()
                    }
                    delay(100)
                }
                runtime.closePty(pty)
                assertTrue(output, output.contains(expected))
                runtime.shutdown()
                runtime.boot()
                assertEquals(marker, command("cat \"\$HOME/$marker\"; cp \"\$HOME/$marker\" \"\$HOME/.lingxi-product-acceptance-cold-$marker\"; rm \"\$HOME/$marker\""))
                android.util.Log.i("ProductAcceptance", "PRODUCT_MOBILE_LINUX_ACCEPTANCE PASS android uid=1000 home=${directories.guestHome} workspace=${directories.defaultGuestCwd} project=${project.record.id}")
            } finally {
                runtime.shutdown()
                fixture.deleteRecursively()
            }
        }
        instrumentation.addResults(android.os.Bundle().apply { putString("mobileLinuxProductResult", "READY") })
    }
}

/** Run separately after force-stopping the product so it verifies a new process. */
@RunWith(AndroidJUnit4::class)
class MobileLinuxProductColdStartAcceptanceTest {
    @Test
    fun coldProcessRetainsUserHome() = runBlocking<Unit> {
        requireAcceptanceOptIn(
            "mobileLinuxColdStartAcceptance", "Cold start requires the preceding product acceptance and a stopped process",
            "com.lingxi.code.runtime.MobileLinuxProductColdStartAcceptanceTest#coldProcessRetainsUserHome",
        )
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        ActivityScenario.launch(MainActivity::class.java).use {
            val config = LinuxRuntimeBridge.configForWorkspace(context, LinuxRuntimeMode.MobileLinux)
            val runtime = buildAndroidMobileLinuxRuntimeHandle(config)
            runtime.boot()
            val result = runtime.runCommand(MobileLinuxCommandRequestFfi(
                command = "/bin/sh", args = listOf("-lc", "set -e; found=0; for file in \"\$HOME\"/.lingxi-product-acceptance-cold-*; do [ -f \"\$file\" ] || continue; value=\$(cat \"\$file\"); [ \"\$file\" = \"\$HOME/.lingxi-product-acceptance-cold-\$value\" ]; rm \"\$file\"; found=1; done; [ \"\$found\" = 1 ]; printf 'PRODUCT_COLD_%s\\n' OK"),
                cwd = null, env = emptyList(), stdin = null, timeoutMs = 30_000uL,
                allowNetwork = false, mounts = emptyList(),
            ))
            assertEquals(result.stderr, 0, result.exitCode)
            assertEquals("PRODUCT_COLD_OK", result.stdout.trim())
            runtime.shutdown()
            android.util.Log.i("ProductAcceptance", "PRODUCT_COLD_START_ACCEPTANCE PASS android")
        }
        instrumentation.addResults(android.os.Bundle().apply { putString("mobileLinuxColdStartResult", "VERIFIED") })
    }
}
