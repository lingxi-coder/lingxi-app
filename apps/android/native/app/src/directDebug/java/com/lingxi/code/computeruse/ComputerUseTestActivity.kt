package com.lingxi.code.computeruse

import android.app.Activity
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Bundle
import android.text.InputType
import android.view.View
import android.view.WindowManager
import android.webkit.WebView
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.lingxi.code.R

/**
 * Direct-debug-only fixture for Accessibility and MediaProjection instrumented
 * tests. It is intentionally absent from Direct release and every Play APK.
 */
class ComputerUseTestActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent.getBooleanExtra(EXTRA_FLAG_SECURE, false)) {
            window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        }
        setContentView(buildFixture())
    }

    private fun buildFixture(): View = ScrollView(this).apply {
        id = R.id.computer_use_test_scroll
        addView(
            LinearLayout(context).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(32, 32, 32, 32)
                addView(EditText(context).apply {
                    id = R.id.computer_use_test_text
                    hint = "Automation text"
                    inputType = InputType.TYPE_CLASS_TEXT
                })
                addView(EditText(context).apply {
                    id = R.id.computer_use_test_password
                    hint = "Password"
                    inputType =
                        InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                })
                repeat(30) { index ->
                    addView(TextView(context).apply {
                        text = "Scrollable row $index"
                        textSize = 18f
                        setPadding(16, 24, 16, 24)
                    })
                }
                addView(WebView(context).apply {
                    id = R.id.computer_use_test_webview
                    loadData(
                        "<html><body><button>Web action</button></body></html>",
                        "text/html",
                        "UTF-8",
                    )
                }, LinearLayout.LayoutParams(-1, 320))
                addView(TestCanvas(context).apply {
                    id = R.id.computer_use_test_canvas
                    contentDescription = "Custom drawn target"
                }, LinearLayout.LayoutParams(-1, 320))
            },
        )
    }

    private class TestCanvas(context: android.content.Context) : View(context) {
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = Color.rgb(112, 87, 255)
            textSize = 48f
        }

        override fun onDraw(canvas: Canvas) {
            super.onDraw(canvas)
            canvas.drawText("Canvas target", 32f, 120f, paint)
        }
    }

    companion object {
        const val EXTRA_FLAG_SECURE = "flag_secure"
    }
}
