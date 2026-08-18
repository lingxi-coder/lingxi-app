package com.lingxi.code.localapps

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import com.lingxi.code.localapps.widget.LocalAppWidgetPinRequester
import com.lingxi.code.localapps.widget.LocalAppWidgetSnapshot
import com.lingxi.code.localapps.widget.LocalAppWidgetSnapshotStore
import com.lingxi.code.localapps.widget.ConfigureAction
import com.lingxi.code.localapps.widget.ConfigureStep
import com.lingxi.code.localapps.widget.WidgetOwnership
import com.lingxi.code.localapps.widget.canConfigureWidget
import com.lingxi.code.localapps.widget.decideConfigureStep
import com.lingxi.code.localapps.widget.remappedWidgetBindings
import com.lingxi.code.localapps.widget.widgetOwnership
import com.lingxi.code.localapps.widget.resolveConfigureAppId
import com.lingxi.code.localapps.widget.resolvePinnedWidgetBind
import com.lingxi.code.localapps.widget.shouldRetractPinBinding
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LocalAppWidgetBindingTest {
    @Test
    fun `pin callback flags stay mutable so the launcher can fill EXTRA_APPWIDGET_ID`() {
        val flags = LocalAppWidgetPinRequester.PIN_CALLBACK_FLAGS
        assertEquals(0, flags and PendingIntent.FLAG_IMMUTABLE)
        assertEquals(PendingIntent.FLAG_MUTABLE, flags and PendingIntent.FLAG_MUTABLE)
        assertEquals(
            PendingIntent.FLAG_UPDATE_CURRENT,
            flags and PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    @Test
    fun `pin receiver binds only when the system fill-in supplies a widget id`() {
        assertNull(
            resolvePinnedWidgetBind(
                appId = "tracker-1",
                widgetId = AppWidgetManager.INVALID_APPWIDGET_ID,
            ),
        )
        assertNull(resolvePinnedWidgetBind(appId = "../unsafe", widgetId = 42))

        val bind = resolvePinnedWidgetBind(appId = "tracker-1", widgetId = 42)
        assertEquals("tracker-1", bind?.appId)
        assertEquals(42, bind?.widgetId)
    }

    @Test
    fun `configure retries widget-scoped options long enough for the pin receiver`() {
        assertTrue(LocalAppWidgetPinRequester.OPTIONS_RETRY_ATTEMPTS >= 4)
        assertTrue(LocalAppWidgetPinRequester.OPTIONS_RETRY_INTERVAL_MS in 50L..300L)
    }

    @Test
    fun `configure auto-finishes from an existing binding or this widget's extras`() {
        assertEquals(
            "tracker-1",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7, 8),
                existingAppId = "tracker-1",
            ),
        )
        assertEquals(
            "notes-2",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = null,
                widgetScopedAppId = "notes-2",
            ),
        )
        assertEquals(
            "tracker-1",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = "tracker-1",
                widgetScopedAppId = "notes-2",
            ),
        )
        assertNull(
            "a home-screen add without extras must show the picker",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = null,
            ),
        )
        assertNull(
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(8),
                existingAppId = "tracker-1",
                widgetScopedAppId = "notes-2",
            ),
        )
        assertNull(
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = "Tracker",
                widgetScopedAppId = "../unsafe",
            ),
        )
    }

    @Test
    fun `configure rejects widget ids that do not belong to this provider`() {
        assertFalse(canConfigureWidget(widgetId = 7, ownedWidgetIds = intArrayOf()))
        assertFalse(
            canConfigureWidget(
                widgetId = AppWidgetManager.INVALID_APPWIDGET_ID,
                ownedWidgetIds = intArrayOf(AppWidgetManager.INVALID_APPWIDGET_ID),
            ),
        )
        assertFalse(canConfigureWidget(widgetId = 7, ownedWidgetIds = intArrayOf(8, 9)))
        assertTrue(canConfigureWidget(widgetId = 7, ownedWidgetIds = intArrayOf(7, 8)))
    }

    @Test
    fun `configure wait retries then finishes without showing the picker`() {
        assertEquals(
            ConfigureStep(ConfigureAction.Retry),
            decideConfigureStep(
                ownership = WidgetOwnership.Wait,
                existingAppId = "tracker-1",
                widgetScopedAppId = "notes-2",
                retriesRemaining = true,
            ),
        )
        assertEquals(
            ConfigureStep(ConfigureAction.Finish),
            decideConfigureStep(
                ownership = WidgetOwnership.Wait,
                existingAppId = "tracker-1",
                widgetScopedAppId = "notes-2",
                retriesRemaining = false,
            ),
        )
    }

    @Test
    fun `configure accept auto-binds from store or options and otherwise shows the picker`() {
        assertEquals(
            ConfigureStep(ConfigureAction.AutoBind, "tracker-1"),
            decideConfigureStep(
                ownership = WidgetOwnership.Accept,
                existingAppId = "tracker-1",
                widgetScopedAppId = "notes-2",
                retriesRemaining = true,
            ),
        )
        assertEquals(
            ConfigureStep(ConfigureAction.AutoBind, "notes-2"),
            decideConfigureStep(
                ownership = WidgetOwnership.Accept,
                existingAppId = null,
                widgetScopedAppId = "notes-2",
                retriesRemaining = false,
            ),
        )
        assertEquals(
            ConfigureStep(ConfigureAction.ShowPicker),
            decideConfigureStep(
                ownership = WidgetOwnership.Accept,
                existingAppId = null,
                retriesRemaining = true,
            ),
        )
        assertEquals(
            ConfigureStep(ConfigureAction.ShowPicker),
            decideConfigureStep(
                ownership = WidgetOwnership.Accept,
                existingAppId = "Tracker",
                widgetScopedAppId = "../unsafe",
                retriesRemaining = false,
            ),
        )
    }

    @Test
    fun `configure reject always finishes`() {
        assertEquals(
            ConfigureStep(ConfigureAction.Finish),
            decideConfigureStep(
                ownership = WidgetOwnership.Reject,
                existingAppId = "tracker-1",
                retriesRemaining = true,
            ),
        )
    }

    @Test
    fun `pin binding retracts only when the widget never becomes owned`() {
        assertTrue(
            LocalAppWidgetPinRequester.UNOWNED_BINDING_RETRACT_MS >
                LocalAppWidgetPinRequester.OPTIONS_RETRY_ATTEMPTS *
                LocalAppWidgetPinRequester.OPTIONS_RETRY_INTERVAL_MS,
        )
        assertFalse(
            shouldRetractPinBinding(
                ownership = WidgetOwnership.Accept,
                storedAppId = "tracker-1",
                writtenAppId = "tracker-1",
            ),
        )
        assertTrue(
            shouldRetractPinBinding(
                ownership = WidgetOwnership.Wait,
                storedAppId = "tracker-1",
                writtenAppId = "tracker-1",
            ),
        )
        assertFalse(
            "a later user or configure bind must not be deleted",
            shouldRetractPinBinding(
                ownership = WidgetOwnership.Wait,
                storedAppId = "notes-2",
                writtenAppId = "tracker-1",
            ),
        )
        assertFalse(
            shouldRetractPinBinding(
                ownership = WidgetOwnership.Wait,
                storedAppId = null,
                writtenAppId = "tracker-1",
            ),
        )
    }

    @Test
    fun `a valid id missing from the owned list is a wait not a reject`() {
        assertEquals(
            WidgetOwnership.Reject,
            widgetOwnership(AppWidgetManager.INVALID_APPWIDGET_ID, intArrayOf(1)),
        )
        assertEquals(WidgetOwnership.Accept, widgetOwnership(7, intArrayOf(7, 8)))
        assertEquals(
            "an empty owned list means the new id may not be registered yet",
            WidgetOwnership.Wait,
            widgetOwnership(7, intArrayOf()),
        )
        assertEquals(
            "existing widgets must not make a just-allocated id look foreign",
            WidgetOwnership.Wait,
            widgetOwnership(7, intArrayOf(8, 9)),
        )
    }

    @Test
    fun `configure may auto-bind from widget-scoped options only after the widget is owned`() {
        assertEquals(
            "notes-2",
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(7),
                existingAppId = null,
                widgetScopedAppId = "notes-2",
            ),
        )
        assertNull(
            resolveConfigureAppId(
                widgetId = 7,
                ownedWidgetIds = intArrayOf(8),
                existingAppId = null,
                widgetScopedAppId = "notes-2",
            ),
        )
    }

    @Test
    fun `snapshot decode rejects unsupported versions and drops invalid or duplicate ids`() {
        val unsupported = LocalAppWidgetSnapshotStore.decode("""{"version":2,"apps":[]}""")
        assertEquals(LocalAppWidgetSnapshot(), unsupported)

        val snapshot = LocalAppWidgetSnapshotStore.decode(
            """
            {"version":1,"apps":[
                {"id":"tracker-1","name":"First","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":1},
                {"id":"tracker-1","name":"Duplicate","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":2},
                {"id":"../unsafe","name":"Unsafe","brief":"","workflow":"ready","runtimeState":"stopped","updatedAtMs":3}
            ]}
            """.trimIndent(),
        )
        assertEquals(listOf("tracker-1"), snapshot.apps.map { it.id })
        assertEquals("First", snapshot.apps.single().name)
    }

    @Test
    fun `restored widget ids remap stored app bindings`() {
        val remapped = remappedWidgetBindings(
            oldIds = intArrayOf(3, 7),
            newIds = intArrayOf(11, 13),
            current = mapOf(3 to "tracker-1", 7 to "notes-2", 9 to "keep-me"),
        )
        assertEquals(
            mapOf(11 to "tracker-1", 13 to "notes-2", 9 to "keep-me"),
            remapped,
        )
        assertTrue(remapped.none { it.key == 3 || it.key == 7 })
    }

    @Test
    fun `restored widget ids keep bindings when old and new ids overlap`() {
        val remapped = remappedWidgetBindings(
            oldIds = intArrayOf(1, 2),
            newIds = intArrayOf(2, 3),
            current = mapOf(1 to "app-a", 2 to "app-b"),
        )
        assertEquals(mapOf(2 to "app-a", 3 to "app-b"), remapped)
    }

    @Test
    fun `pin request codes stay unique per app id`() {
        assertEquals(
            LocalAppWidgetPinRequester.requestCodeFor("tracker-1"),
            LocalAppWidgetPinRequester.requestCodeFor("tracker-1"),
        )
        assertNotEquals(
            LocalAppWidgetPinRequester.requestCodeFor("tracker-1"),
            LocalAppWidgetPinRequester.requestCodeFor("notes-2"),
        )
    }
}
