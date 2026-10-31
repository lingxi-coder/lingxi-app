package com.lingxi.code.apps

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LocalAppBrowserIntentTest {
    @Test fun defaultAndChooserKeepTheTargetImplicit() {
        val url = "https://example.org/open#ticket"
        val direct = LocalAppBrowser.intent(url)
        assertEquals(Intent.ACTION_VIEW, direct.action)
        assertNull(direct.`package`)
        assertNull(direct.component)
        assertTrue(direct.hasCategory(Intent.CATEGORY_BROWSABLE))
        val chooser = LocalAppBrowser.chooserIntent(url)
        assertEquals(Intent.ACTION_CHOOSER, chooser.action)
        @Suppress("DEPRECATION")
        val target = chooser.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)!!
        assertEquals(direct.data, target.data)
        assertNull(target.`package`)
        assertNull(target.component)
    }
}
