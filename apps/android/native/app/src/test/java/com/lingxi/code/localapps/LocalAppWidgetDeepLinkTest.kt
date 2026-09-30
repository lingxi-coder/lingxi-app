package com.lingxi.code.localapps

import com.lingxi.code.device.escapeLikePattern
import com.lingxi.code.device.parseCalendarQuery
import com.lingxi.code.localapps.widget.LocalAppWidgetDeepLink
import com.lingxi.code.localapps.widget.LocalAppWidgetPinRequester
import com.lingxi.code.localapps.widget.resolveConfigureAppId
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppWidgetDeepLinkTest {
    @Test
    fun `widget deep link accepts valid parts and trims autostart`() {
        val request = LocalAppWidgetDeepLink.parseParts(
            scheme = "LINGXI",
            host = "OPEN_LOCAL_APP",
            path = "/",
            parameters = mapOf(
                "appId" to " tracker-1 ",
                "destination" to " preview ",
                "autostart" to " 0 ",
                "source" to " widget ",
            ),
        )

        assertEquals("tracker-1", request?.appId)
        assertFalse(request?.autostart == true)
        assertEquals("widget", request?.source)
    }

    @Test
    fun `widget deep link rejects unsafe path and unknown query`() {
        assertNull(
            LocalAppWidgetDeepLink.parseParts(
                scheme = "lingxi",
                host = "open_local_app",
                path = "/tmp/secret",
                parameters = mapOf("appId" to "tracker-1", "destination" to "preview"),
            ),
        )
        assertNull(
            LocalAppWidgetDeepLink.parseParts(
                scheme = "lingxi",
                host = "open_local_app",
                path = null,
                parameters = mapOf(
                    "appId" to "tracker-1",
                    "destination" to "preview",
                    "path" to "/tmp/secret",
                ),
            ),
        )
    }

    @Test
    fun `widget app id validation stays fail closed`() {
        assertTrue(LocalAppWidgetDeepLink.isValidAppId("tracker-1"))
        assertFalse(LocalAppWidgetDeepLink.isValidAppId("Tracker"))
        assertFalse(LocalAppWidgetDeepLink.isValidAppId("a".repeat(65)))
    }

    @Test
    fun `configure without this widget's extras stays on the picker`() {
        assertNull(LocalAppWidgetPinRequester.appIdFromOptions(null))
        assertNull(
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = null,
            ),
        )
        assertEquals(
            "tracker-1",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = "tracker-1",
            ),
        )
    }

    @Test
    fun `calendar query decodes the host snake_case contract`() {
        val query = parseCalendarQuery("""{"start_ms":1000,"end_ms":2000,"limit":8}""")
        assertEquals(1000L, query.startMs)
        assertEquals(2000L, query.endMs)
        assertEquals(8, query.limit)
    }

    @Test
    fun `contacts like pattern escapes wildcards`() {
        assertEquals("A\\_B\\%C\\\\D", escapeLikePattern("A_B%C\\D"))
    }
}
