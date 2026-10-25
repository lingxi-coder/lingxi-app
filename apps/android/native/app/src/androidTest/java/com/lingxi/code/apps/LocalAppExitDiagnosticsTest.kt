package com.lingxi.code.apps

import android.app.ActivityManager
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import java.io.File

/** Explicitly selected diagnostic helper; never touches another package's traces. */
class LocalAppExitDiagnosticsTest {
    @Test fun exportOwnAnr() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val exits = context.getSystemService(ActivityManager::class.java).getHistoricalProcessExitReasons(context.packageName, 0, 16)
        val exit = exits.firstOrNull { it.reason == android.app.ApplicationExitInfo.REASON_ANR } ?: return
        exit.traceInputStream?.use { trace ->
            File(context.filesDir, "local-app-acceptance-anr.txt").outputStream().use { output ->
                val bytes = trace.readNBytes(2 * 1024 * 1024); output.write(bytes)
            }
        }
    }
}
