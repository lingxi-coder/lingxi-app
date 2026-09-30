package com.lingxi.code.computeruse

import android.content.Intent
import android.text.InputType
import android.view.WindowManager
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.espresso.Espresso.onView
import androidx.test.espresso.action.ViewActions.replaceText
import androidx.test.espresso.assertion.ViewAssertions.matches
import androidx.test.espresso.matcher.ViewMatchers.isDisplayed
import androidx.test.espresso.matcher.ViewMatchers.withId
import androidx.test.espresso.matcher.ViewMatchers.withInputType
import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.R
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ComputerUseTestActivityTest {
    @Test
    fun fixture_exposes_editable_password_scroll_webview_and_canvas_targets() {
        ActivityScenario.launch(ComputerUseTestActivity::class.java).use {
            onView(withId(R.id.computer_use_test_text))
                .perform(replaceText("UTF-8 输入 ✓"))
                .check(matches(withInputType(InputType.TYPE_CLASS_TEXT)))
            onView(withId(R.id.computer_use_test_password))
                .check(
                    matches(
                        withInputType(
                            InputType.TYPE_CLASS_TEXT or
                                InputType.TYPE_TEXT_VARIATION_PASSWORD,
                        ),
                    ),
                )
            onView(withId(R.id.computer_use_test_scroll)).check(matches(isDisplayed()))
            onView(withId(R.id.computer_use_test_webview)).check(matches(isDisplayed()))
            onView(withId(R.id.computer_use_test_canvas)).check(matches(isDisplayed()))
        }
    }

    @Test
    fun fixture_can_present_a_flag_secure_window() {
        val context = ApplicationProvider.getApplicationContext<android.content.Context>()
        val intent = Intent(context, ComputerUseTestActivity::class.java)
            .putExtra(ComputerUseTestActivity.EXTRA_FLAG_SECURE, true)
        ActivityScenario.launch<ComputerUseTestActivity>(intent).use { scenario ->
            scenario.onActivity { activity ->
                assert(
                    activity.window.attributes.flags and
                        WindowManager.LayoutParams.FLAG_SECURE != 0,
                )
            }
        }
    }
}
