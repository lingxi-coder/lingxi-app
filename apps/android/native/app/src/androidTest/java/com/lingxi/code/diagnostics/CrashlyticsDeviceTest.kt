package com.lingxi.code.diagnostics

import android.os.Handler
import android.os.Looper
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.google.firebase.FirebaseApp
import com.google.firebase.crashlytics.FirebaseCrashlytics
import com.lingxi.code.BuildConfig
import java.util.UUID
import java.util.concurrent.TimeUnit
import com.google.android.gms.tasks.Tasks
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** Explicitly selected device checks only. The default suite skips both methods. */
@RunWith(AndroidJUnit4::class)
class CrashlyticsDeviceTest {
    private external fun crashNative()

    private fun reporter(): FirebaseCrashlytics {
        val args = InstrumentationRegistry.getArguments()
        assumeTrue(args.getString("liveCrashlyticsProject") == "lingxi-5e4b2")
        check(BuildConfig.DEBUG && FirebaseApp.getInstance().options.projectId == "lingxi-5e4b2")
        return FirebaseCrashlytics.getInstance()
    }

    @Test
    fun captureFatalCrash() {
        val reporter = reporter()
        val session = UUID.fromString(InstrumentationRegistry.getArguments().getString("verificationId")).toString()
        check(!reporter.isCrashlyticsCollectionEnabled)
        reporter.setCustomKey("verification_id", session)
        Handler(Looper.getMainLooper()).post { throw CrashlyticsVerificationException() }
        // The unhandled main-thread exception intentionally terminates this runner.
        Thread.sleep(30_000)
        error("The verification crash did not terminate the app")
    }

    @Test
    fun captureNativeFatalCrash() {
        val reporter = reporter()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val session = UUID.fromString(InstrumentationRegistry.getArguments().getString("verificationId")).toString()
        check(!reporter.isCrashlyticsCollectionEnabled)
        check(!Tasks.await(reporter.checkForUnsentReports(), 30, TimeUnit.SECONDS))
        val library = java.io.File(instrumentation.targetContext.codeCacheDir, "libcrashlytics_verification.so")
        instrumentation.context.assets.open("crashlytics/arm64-v8a/libcrashlytics_verification.so").use { input ->
            library.outputStream().use { output -> input.copyTo(output) }
        }
        System.load(library.absolutePath)
        reporter.setCustomKey("verification_id", session)
        Thread.sleep(5_000)
        crashNative()
        error("The native verification crash did not terminate the app")
    }

    @Test
    fun sendCapturedReport() {
        val reporter = reporter()
        check(!reporter.isCrashlyticsCollectionEnabled)
        check(Tasks.await(reporter.checkForUnsentReports(), 30, TimeUnit.SECONDS))
        // One-shot consent leaves the normal debug collection policy disabled.
        reporter.sendUnsentReports()
        Thread.sleep(10_000)
    }

    private class CrashlyticsVerificationException : RuntimeException("Intentional Crashlytics integration verification")
}
