package com.lingxi.code.device

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.os.VibrationEffect
import android.os.Vibrator
import android.provider.CalendarContract
import android.provider.ContactsContract
import com.lingxi.code.bindings.AndroidDeviceControl
import com.lingxi.code.bindings.DeviceControlFfiException
import androidx.core.content.ContextCompat
import org.json.JSONObject
import org.json.JSONArray

/** Process-global Android implementation of Local App device controls. */
object AndroidDeviceControlController {
    @Volatile private var context: Context? = null

    fun attach(context: Context) {
        this.context = context.applicationContext
    }

    fun detach() {
        context = null
    }

    fun statusJson(): String {
        val ctx = context ?: throw DeviceControlFfiException.Unavailable()
        val battery = ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = battery?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = battery?.getIntExtra(BatteryManager.EXTRA_SCALE, -1) ?: -1
        val percent = if (level >= 0 && scale > 0) level * 100.0 / scale else null
        val status = battery?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val charging = when (status) {
            BatteryManager.BATTERY_STATUS_CHARGING,
            BatteryManager.BATTERY_STATUS_FULL -> true
            BatteryManager.BATTERY_STATUS_DISCHARGING,
            BatteryManager.BATTERY_STATUS_NOT_CHARGING -> false
            else -> null
        }
        val connectivity = ctx.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val capabilities = connectivity.getNetworkCapabilities(connectivity.activeNetwork)
        val network = when {
            capabilities == null -> "offline"
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "online"
        }
        val power = ctx.getSystemService(Context.POWER_SERVICE) as PowerManager
        return JSONObject()
            .put("batteryPercent", percent)
            .put("charging", charging)
            .put("network", network)
            .put("lowPowerMode", if (Build.VERSION.SDK_INT >= 21) power.isPowerSaveMode else null)
            .toString()
    }

    fun triggerHaptic(style: String) {
        val ctx = context ?: throw DeviceControlFfiException.Unavailable()
        val vibrator = ctx.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
            ?: throw DeviceControlFfiException.Unavailable()
        val (duration, amplitude) = when (style) {
            "light" -> 18L to 45
            "medium" -> 28L to 100
            "heavy" -> 42L to 180
            "success" -> 20L to 90
            "warning" -> 30L to 140
            "error" -> 50L to 220
            else -> throw DeviceControlFfiException.Rejected("unsupported haptic style")
        }
        if (Build.VERSION.SDK_INT >= 26) {
            vibrator.vibrate(VibrationEffect.createOneShot(duration, amplitude))
        } else {
            @Suppress("DEPRECATION")
            vibrator.vibrate(duration)
        }
    }

    fun openDeepLink(url: String) {
        val ctx = context ?: throw DeviceControlFfiException.Unavailable()
        try {
            ctx.startActivity(
                Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        } catch (t: Throwable) {
            throw DeviceControlFfiException.Other(t.message ?: "deep link launch failed")
        }
    }

    fun calendarJson(requestJson: String): String {
        val ctx = context ?: throw DeviceControlFfiException.Unavailable()
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.READ_CALENDAR) != PackageManager.PERMISSION_GRANTED) {
            throw DeviceControlFfiException.Rejected("calendar permission denied")
        }
        val request = parseCalendarQuery(requestJson)
        val startMs = request.startMs
        val endMs = request.endMs
        val limit = request.limit
        val uri = CalendarContract.Instances.CONTENT_URI.buildUpon()
            .appendPath(startMs.toString())
            .appendPath(endMs.toString())
            .build()
        val projection = arrayOf(
            CalendarContract.Instances.EVENT_ID,
            CalendarContract.Instances.TITLE,
            CalendarContract.Instances.BEGIN,
            CalendarContract.Instances.END,
            CalendarContract.Instances.ALL_DAY,
            CalendarContract.Instances.EVENT_LOCATION,
            CalendarContract.Instances.DESCRIPTION,
            CalendarContract.Instances.CALENDAR_DISPLAY_NAME,
        )
        return try {
            val result = JSONArray()
            ctx.contentResolver.query(uri, projection, null, null, "${CalendarContract.Instances.BEGIN} ASC")?.use { cursor ->
                val eventId = cursor.getColumnIndexOrThrow(CalendarContract.Instances.EVENT_ID)
                val title = cursor.getColumnIndexOrThrow(CalendarContract.Instances.TITLE)
                val begin = cursor.getColumnIndexOrThrow(CalendarContract.Instances.BEGIN)
                val end = cursor.getColumnIndexOrThrow(CalendarContract.Instances.END)
                val allDay = cursor.getColumnIndexOrThrow(CalendarContract.Instances.ALL_DAY)
                val location = cursor.getColumnIndexOrThrow(CalendarContract.Instances.EVENT_LOCATION)
                val description = cursor.getColumnIndexOrThrow(CalendarContract.Instances.DESCRIPTION)
                val calendar = cursor.getColumnIndexOrThrow(CalendarContract.Instances.CALENDAR_DISPLAY_NAME)
                while (cursor.moveToNext() && result.length() < limit) {
                    result.put(JSONObject()
                        .put("id", cursor.getString(eventId))
                        .put("title", cursor.getString(title) ?: "")
                        .put("start_ms", cursor.getLong(begin))
                        .put("end_ms", cursor.getLong(end))
                        .put("all_day", cursor.getInt(allDay) != 0)
                        .put("location", cursor.getString(location))
                        .put("notes", cursor.getString(description))
                        .put("calendar", cursor.getString(calendar)))
                }
            }
            result.toString()
        } catch (security: SecurityException) {
            throw DeviceControlFfiException.Rejected("calendar permission denied")
        } catch (t: Throwable) {
            throw DeviceControlFfiException.Other(t.message ?: "calendar query failed")
        }
    }

    fun contactsJson(requestJson: String): String {
        val ctx = context ?: throw DeviceControlFfiException.Unavailable()
        if (ContextCompat.checkSelfPermission(ctx, Manifest.permission.READ_CONTACTS) != PackageManager.PERMISSION_GRANTED) {
            throw DeviceControlFfiException.Rejected("contacts permission denied")
        }
        val request = JSONObject(requestJson)
        val query = request.getString("query").trim()
        val limit = request.optInt("limit", 20).coerceIn(1, 50)
        return try {
            val result = JSONArray()
            val projection = arrayOf(ContactsContract.Contacts._ID, ContactsContract.Contacts.DISPLAY_NAME)
            val selection = "${ContactsContract.Contacts.DISPLAY_NAME} LIKE ? ESCAPE '\\'"
            val args = arrayOf("%${escapeLikePattern(query)}%")
            ctx.contentResolver.query(
                ContactsContract.Contacts.CONTENT_URI,
                projection,
                selection,
                args,
                "${ContactsContract.Contacts.DISPLAY_NAME} COLLATE LOCALIZED ASC",
            )?.use { cursor ->
                val idColumn = cursor.getColumnIndexOrThrow(ContactsContract.Contacts._ID)
                val nameColumn = cursor.getColumnIndexOrThrow(ContactsContract.Contacts.DISPLAY_NAME)
                while (cursor.moveToNext() && result.length() < limit) {
                    val id = cursor.getString(idColumn)
                    val phones = JSONArray()
                    ctx.contentResolver.query(
                        ContactsContract.CommonDataKinds.Phone.CONTENT_URI,
                        arrayOf(ContactsContract.CommonDataKinds.Phone.NUMBER),
                        "${ContactsContract.CommonDataKinds.Phone.CONTACT_ID} = ?",
                        arrayOf(id),
                        null,
                    )?.use { phoneCursor ->
                        val numberColumn = phoneCursor.getColumnIndexOrThrow(ContactsContract.CommonDataKinds.Phone.NUMBER)
                        while (phoneCursor.moveToNext()) phones.put(phoneCursor.getString(numberColumn))
                    }
                    val emails = JSONArray()
                    ctx.contentResolver.query(
                        ContactsContract.CommonDataKinds.Email.CONTENT_URI,
                        arrayOf(ContactsContract.CommonDataKinds.Email.ADDRESS),
                        "${ContactsContract.CommonDataKinds.Email.CONTACT_ID} = ?",
                        arrayOf(id),
                        null,
                    )?.use { emailCursor ->
                        val addressColumn = emailCursor.getColumnIndexOrThrow(ContactsContract.CommonDataKinds.Email.ADDRESS)
                        while (emailCursor.moveToNext()) emails.put(emailCursor.getString(addressColumn))
                    }
                    result.put(JSONObject()
                        .put("id", id)
                        .put("display_name", cursor.getString(nameColumn) ?: "")
                        .put("phones", phones)
                        .put("emails", emails))
                }
            }
            result.toString()
        } catch (security: SecurityException) {
            throw DeviceControlFfiException.Rejected("contacts permission denied")
        } catch (t: Throwable) {
            throw DeviceControlFfiException.Other(t.message ?: "contacts query failed")
        }
    }
}

internal data class CalendarQueryWire(
    val startMs: Long,
    val endMs: Long,
    val limit: Int,
)

internal fun escapeLikePattern(query: String): String =
    query
        .replace("\\", "\\\\")
        .replace("%", "\\%")
        .replace("_", "\\_")

internal fun parseCalendarQuery(requestJson: String): CalendarQueryWire {
    val request = JSONObject(requestJson)
    if (!request.has("start_ms") || !request.has("end_ms")) {
        throw DeviceControlFfiException.Rejected("calendar request must use start_ms/end_ms")
    }
    return CalendarQueryWire(
        startMs = request.getLong("start_ms"),
        endMs = request.getLong("end_ms"),
        limit = request.optInt("limit", 50).coerceIn(1, 100),
    )
}

/** Thin UniFFI callback adapter over [AndroidDeviceControlController]. */
class AndroidDeviceControlAdapter : AndroidDeviceControl {
    override suspend fun statusJson(): String = AndroidDeviceControlController.statusJson()

    override suspend fun triggerHaptic(style: String) {
        AndroidDeviceControlController.triggerHaptic(style)
    }

    override suspend fun openDeepLink(url: String) {
        AndroidDeviceControlController.openDeepLink(url)
    }

    override suspend fun calendarJson(requestJson: String): String =
        AndroidDeviceControlController.calendarJson(requestJson)

    override suspend fun contactsJson(requestJson: String): String =
        AndroidDeviceControlController.contactsJson(requestJson)
}
