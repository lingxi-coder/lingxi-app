package com.lingxi.code.apps

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import com.lingxi.code.theme.LingXiTheme

/** Non-exported, direct-debug-only host for the real native application page. */
class LocalAppAcceptanceActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        val appId = requireNotNull(intent.getStringExtra("appId"))
        require(appId.matches(Regex("a[0-9a-f]{32}")))
        val apps = UnifiedLocalApps.get(applicationContext)
        setContent {
            LingXiTheme {
                UnifiedAppRuntimePage(apps, appId, onClose = ::finish, onShowDetails = { finish() })
            }
        }
    }
}
