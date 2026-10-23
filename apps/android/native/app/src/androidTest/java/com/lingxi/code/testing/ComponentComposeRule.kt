package com.lingxi.code.testing

import android.content.Intent
import android.view.ViewGroup
import androidx.compose.ui.platform.AbstractComposeView
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.v2.AndroidComposeTestRule
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleCallback
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import org.junit.rules.TestRule
import org.junit.runner.Description
import org.junit.runners.model.Statement
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Own the component host directly; no cross-process EmptyActivity teardown broadcasts. */
fun createComponentComposeRule() = AndroidComposeTestRule(ComponentHostRule()) { it.activity }

class ComponentHostRule : TestRule {
    lateinit var activity: ComponentActivity
        private set

    override fun apply(base: Statement, description: Description) = object : Statement() {
        override fun evaluate() {
            val instrumentation = InstrumentationRegistry.getInstrumentation()
            val monitor = ActivityLifecycleMonitorRegistry.getInstance()
            val destroyed = CountDownLatch(1)
            val callback = ActivityLifecycleCallback { candidate, stage ->
                if (::activity.isInitialized && candidate === activity && stage == Stage.DESTROYED) {
                    destroyed.countDown()
                }
            }
            monitor.addLifecycleCallback(callback)
            try {
                activity = instrumentation.startActivitySync(
                    Intent(instrumentation.targetContext, ComponentActivity::class.java)
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
                ) as ComponentActivity
                instrumentation.runOnMainSync {
                    activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
                base.evaluate()
            } finally {
                try {
                    if (::activity.isInitialized) {
                        instrumentation.runOnMainSync {
                            // Dispose owned dialogs before finishing their Activity. Waiting for
                            // Activity destruction first leaves their Compose lifecycle attached.
                            val content = activity.findViewById<ViewGroup>(android.R.id.content)
                            for (index in 0 until content.childCount) {
                                (content.getChildAt(index) as? AbstractComposeView)?.disposeComposition()
                            }
                            activity.finish()
                        }
                        check(destroyed.await(10, TimeUnit.SECONDS)) { "Component host did not destroy" }
                    }
                } finally {
                    monitor.removeLifecycleCallback(callback)
                }
            }
        }
    }
}
