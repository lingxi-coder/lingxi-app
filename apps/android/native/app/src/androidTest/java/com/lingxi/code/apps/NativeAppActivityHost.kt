package com.lingxi.code.apps

import android.content.Intent
import android.view.ViewGroup
import androidx.activity.ComponentActivity
import androidx.compose.ui.platform.AbstractComposeView
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleCallback
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/** Own the activity through rotations and dispose Compose before finishing it.
 * AndroidX ActivityScenario's cross-process EmptyActivity teardown can hang on
 * this device while a native picker/Compose view is still attached.
 */
internal class NativeAppActivityHost(intent: Intent) : AutoCloseable {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val monitor = ActivityLifecycleMonitorRegistry.getInstance()
    private val current = AtomicReference<ComponentActivity>()
    private val destroyed = CountDownLatch(1)
    private var closing = false
    private val name = intent.component!!.className
    private val callback = ActivityLifecycleCallback { candidate, stage ->
        if (candidate.javaClass.name == name && candidate is ComponentActivity) {
            if (stage == Stage.CREATED) current.set(candidate)
            if (closing && candidate === current.get() && stage == Stage.DESTROYED) destroyed.countDown()
        }
    }
    init {
        monitor.addLifecycleCallback(callback)
        try {
            current.set(instrumentation.startActivitySync(intent) as ComponentActivity)
        } catch (failure: Throwable) {
            monitor.removeLifecycleCallback(callback)
            throw failure
        }
    }
    fun bringToForeground() = instrumentation.runOnMainSync {
        val activity = checkNotNull(current.get())
        activity.startActivity(Intent(activity, activity.javaClass).putExtras(activity.intent)
            .addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT or Intent.FLAG_ACTIVITY_SINGLE_TOP))
    }
    fun isResumed(): Boolean {
        var result = false
        instrumentation.runOnMainSync { result = current.get()?.lifecycle?.currentState == androidx.lifecycle.Lifecycle.State.RESUMED }
        return result
    }
    override fun close() {
        try {
            instrumentation.runOnMainSync {
                closing = true
                val activity = current.get() ?: return@runOnMainSync
                val content = activity.findViewById<ViewGroup>(android.R.id.content)
                for (index in 0 until content.childCount) (content.getChildAt(index) as? AbstractComposeView)?.disposeComposition()
                activity.finish()
            }
            check(destroyed.await(10, TimeUnit.SECONDS)) { "Native app test host did not destroy" }
        } finally { monitor.removeLifecycleCallback(callback) }
    }
}
