package com.lingxi.code.navigation

import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.lingxi.code.MainActivity
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/** Verify the installed manifest as well as the pure link parser. */
@RunWith(AndroidJUnit4::class)
class MobileLinkIntentTest {
    @Test fun canonicalMobileLinksResolveThroughTheInstalledIntentFilters() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val routes = listOf("new", "cron/job/test-job", "settings/appearance", "remote/test-host/test-session",
            "team/inbox", "team/join?code=0123456789abcdef0123456789abcdef", "import/mcp")
        for (route in routes) for (origin in listOf("lingxi://", "https://lingxios.net/open/")) {
            val link = origin + route
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(link)).addCategory(Intent.CATEGORY_BROWSABLE)
                .setPackage(context.packageName)
            @Suppress("DEPRECATION")
            val resolved = context.packageManager.queryIntentActivities(intent, PackageManager.MATCH_DEFAULT_ONLY)
            assertEquals(link, listOf(MainActivity::class.java.name), resolved.map { it.activityInfo.name }.distinct())
        }
    }
}
