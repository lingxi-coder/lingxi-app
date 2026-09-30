package com.lingxi.code.offload

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.net.Uri
import android.provider.AlarmClock
import android.provider.CalendarContract
import android.provider.ContactsContract
import android.provider.MediaStore
import androidx.core.content.ContextCompat
import com.lingxi.code.cron.AndroidCronRepository
import com.lingxi.code.voice.audio.AudioOwnerKey
import com.lingxi.code.voice.audio.DeviceAudioResult
import com.lingxi.code.voice.audio.OffloadMediaCommand
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.io.File
import java.security.MessageDigest
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/** Real Android-backed command ports available to both distributions. */
internal object AndroidSystemOffloadPorts {
    fun create(context: Context): Map<String, NativeCommandPort> {
        val app = context.applicationContext
        val audio = AndroidOffloadAudioService(app)
        return mapOf(
            "alarm" to AlarmPort(app),
            "calendar" to CalendarPort(app),
            "contacts" to ContactsPort(app),
            "location" to LocationPort(app),
            "open" to OpenPort(app),
            "photos" to PhotosPort(app),
            "player" to PlayerPort(audio),
            "speech" to SpeechPort(audio),
            "scheduled" to ScheduledPort(app),
        )
    }
}

private class AlarmPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        return when (request.arguments.firstOrNull()) {
            "list" -> launch(Intent(AlarmClock.ACTION_SHOW_ALARMS))
            "set" -> {
                val args = CommandArguments.parse(request.arguments.drop(1))
                    ?: return NativeOffloadResult.usage("alarm set --hour <0-23> --minute <0-59> [--message <text>]")
                val hour = args.option("hour")?.toIntOrNull()
                    ?: return NativeOffloadResult.usage("alarm: valid --hour is required")
                val minute = args.option("minute")?.toIntOrNull()
                    ?: return NativeOffloadResult.usage("alarm: valid --minute is required")
                if (hour !in 0..23 || minute !in 0..59) {
                    return NativeOffloadResult.usage("alarm: hour/minute out of range")
                }
                launch(
                    Intent(AlarmClock.ACTION_SET_ALARM)
                        .putExtra(AlarmClock.EXTRA_HOUR, hour)
                        .putExtra(AlarmClock.EXTRA_MINUTES, minute)
                        .putExtra(AlarmClock.EXTRA_MESSAGE, args.option("message").orEmpty())
                        .putExtra(AlarmClock.EXTRA_SKIP_UI, false),
                )
            }
            "-h", "--help" -> NativeOffloadResult.success(
                "Usage: alarm list | alarm set --hour <0-23> --minute <0-59> [--message <text>]\n",
            )
            else -> NativeOffloadResult.usage(
                "Usage: alarm list | alarm set --hour <0-23> --minute <0-59> [--message <text>]",
            )
        }
    }

    private fun launch(intent: Intent): NativeOffloadResult =
        launchActivity(context, intent, "alarm")
}

private class CalendarPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult =
        when (request.arguments.firstOrNull()) {
            "list" -> list(request.arguments.drop(1))
            "create" -> create(request.arguments.drop(1))
            "-h", "--help" -> NativeOffloadResult.success(
                "Usage: calendar list [--limit <n>] | calendar create --title <text> [--begin-ms <epoch>] [--end-ms <epoch>]\n",
            )
            else -> NativeOffloadResult.usage("Usage: calendar list | calendar create --title <text>")
        }

    private suspend fun list(raw: List<String>): NativeOffloadResult = withContext(Dispatchers.IO) {
        val args = CommandArguments.parse(raw)
            ?: return@withContext NativeOffloadResult.usage("calendar list [--limit <n>]")
        val limit = args.option("limit")?.toIntOrNull()?.coerceIn(1, 100) ?: 20
        val rows = mutableListOf<String>()
        val sort = "${CalendarContract.Events.DTSTART} DESC"
        val cursor = if (android.os.Build.VERSION.SDK_INT >= 26) {
            val extras = android.os.Bundle().apply {
                putInt(android.content.ContentResolver.QUERY_ARG_LIMIT, limit)
                putString(android.content.ContentResolver.QUERY_ARG_SQL_SORT_ORDER, sort)
            }
            context.contentResolver.query(CalendarContract.Events.CONTENT_URI, arrayOf(
                CalendarContract.Events._ID,
                CalendarContract.Events.TITLE,
                CalendarContract.Events.DTSTART,
                CalendarContract.Events.DTEND,
            ), extras, null)
        } else {
            context.contentResolver.query(
                CalendarContract.Events.CONTENT_URI,
                arrayOf(
                    CalendarContract.Events._ID,
                    CalendarContract.Events.TITLE,
                    CalendarContract.Events.DTSTART,
                    CalendarContract.Events.DTEND,
                ),
                null,
                null,
                sort,
            )
        }
        cursor?.use { cursor ->
            val id = cursor.getColumnIndexOrThrow(CalendarContract.Events._ID)
            val title = cursor.getColumnIndexOrThrow(CalendarContract.Events.TITLE)
            val start = cursor.getColumnIndexOrThrow(CalendarContract.Events.DTSTART)
            val end = cursor.getColumnIndexOrThrow(CalendarContract.Events.DTEND)
            while (cursor.moveToNext() && rows.size < limit) {
                rows += listOf(
                    cursor.getLong(id).toString(),
                    clean(cursor.getString(title)),
                    cursor.getLong(start).toString(),
                    cursor.getLong(end).toString(),
                ).joinToString("\t")
            }
        }
        NativeOffloadResult.success(rows.joinToString("\n", postfix = if (rows.isEmpty()) "" else "\n"))
    }

    private fun create(raw: List<String>): NativeOffloadResult {
        val args = CommandArguments.parse(raw)
            ?: return NativeOffloadResult.usage("calendar create --title <text>")
        val title = args.option("title")
            ?: return NativeOffloadResult.usage("calendar: --title is required")
        val intent = Intent(Intent.ACTION_INSERT)
            .setData(CalendarContract.Events.CONTENT_URI)
            .putExtra(CalendarContract.Events.TITLE, title)
        args.option("begin-ms")?.toLongOrNull()?.let {
            intent.putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, it)
        }
        args.option("end-ms")?.toLongOrNull()?.let {
            intent.putExtra(CalendarContract.EXTRA_EVENT_END_TIME, it)
        }
        return launchActivity(context, intent, "calendar")
    }
}

private class ContactsPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult =
        when (request.arguments.firstOrNull()) {
            "list" -> query(null, request.arguments.drop(1))
            "search" -> {
                val term = request.arguments.getOrNull(1)
                    ?: return NativeOffloadResult.usage("contacts search <query>")
                query(term, request.arguments.drop(2))
            }
            "open" -> {
                val id = request.arguments.getOrNull(1)?.toLongOrNull()
                    ?: return NativeOffloadResult.usage("contacts open <id>")
                launchActivity(
                    context,
                    Intent(
                        Intent.ACTION_VIEW,
                        ContentUris.withAppendedId(ContactsContract.Contacts.CONTENT_URI, id),
                    ),
                    "contacts",
                )
            }
            "-h", "--help" -> NativeOffloadResult.success(
                "Usage: contacts list [--limit <n>] | contacts search <query> | contacts open <id>\n",
            )
            else -> NativeOffloadResult.usage("Usage: contacts list | contacts search <query> | contacts open <id>")
        }

    private suspend fun query(term: String?, raw: List<String>): NativeOffloadResult =
        withContext(Dispatchers.IO) {
            val args = CommandArguments.parse(raw)
                ?: return@withContext NativeOffloadResult.usage("contacts: malformed options")
            val limit = args.option("limit")?.toIntOrNull()?.coerceIn(1, 100) ?: 30
            val rows = mutableListOf<String>()
            val selection = term?.let { "${ContactsContract.Contacts.DISPLAY_NAME_PRIMARY} LIKE ?" }
            val selectionArgs = term?.let { arrayOf("%$it%") }
            context.contentResolver.query(
                ContactsContract.Contacts.CONTENT_URI,
                arrayOf(
                    ContactsContract.Contacts._ID,
                    ContactsContract.Contacts.DISPLAY_NAME_PRIMARY,
                    ContactsContract.Contacts.HAS_PHONE_NUMBER,
                ),
                selection,
                selectionArgs,
                "${ContactsContract.Contacts.DISPLAY_NAME_PRIMARY} COLLATE NOCASE",
            )?.use { cursor ->
                val id = cursor.getColumnIndexOrThrow(ContactsContract.Contacts._ID)
                val name = cursor.getColumnIndexOrThrow(ContactsContract.Contacts.DISPLAY_NAME_PRIMARY)
                val hasPhone = cursor.getColumnIndexOrThrow(ContactsContract.Contacts.HAS_PHONE_NUMBER)
                while (cursor.moveToNext() && rows.size < limit) {
                    rows += "${cursor.getLong(id)}\t${clean(cursor.getString(name))}\t${cursor.getInt(hasPhone) != 0}"
                }
            }
            NativeOffloadResult.success(rows.joinToString("\n", postfix = if (rows.isEmpty()) "" else "\n"))
        }
}

private class LocationPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        if (request.arguments.isNotEmpty() && request.arguments != listOf("last")) {
            return NativeOffloadResult.usage("Usage: location [last]")
        }
        val manager = context.getSystemService(Context.LOCATION_SERVICE) as LocationManager
        val fineGranted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.ACCESS_FINE_LOCATION,
        ) == PackageManager.PERMISSION_GRANTED
        val coarseGranted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.ACCESS_COARSE_LOCATION,
        ) == PackageManager.PERMISSION_GRANTED
        if (!fineGranted && !coarseGranted) {
            throw NativeOffloadPermissionException(
                NativeOffloadPermissionError(
                    code = "ANDROID_PERMISSION_DENIED",
                    tool = "location",
                    message = "Android location permission is not granted",
                    recoverable = true,
                ),
            )
        }
        val location = try {
            manager.getProviders(true)
                .mapNotNull { provider -> manager.getLastKnownLocation(provider) }
                .maxByOrNull(Location::getTime)
        } catch (denied: SecurityException) {
            throw NativeOffloadPermissionException(
                NativeOffloadPermissionError(
                    code = "ANDROID_PERMISSION_DENIED",
                    tool = "location",
                    message = denied.message ?: "Android location permission was revoked",
                    recoverable = true,
                ),
            )
        }
            ?: return NativeOffloadResult(
                exitCode = NativeOffloadResult.EXIT_FAILURE,
                stderr = "location: no cached location fix\n".toByteArray(),
            )
        return NativeOffloadResult.success(
            "latitude=${location.latitude}\nlongitude=${location.longitude}\n" +
                "accuracyMeters=${location.accuracy}\ntimeMs=${location.time}\nprovider=${location.provider}\n",
        )
    }
}

private class OpenPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        val target = request.arguments.firstOrNull()
            ?: return NativeOffloadResult.usage("open <https://...|mailto:...|tel:...|geo:...>")
        val uri = runCatching { Uri.parse(target) }.getOrNull()
            ?: return NativeOffloadResult.usage("open: invalid URI")
        if (uri.scheme?.lowercase() !in setOf("http", "https", "mailto", "tel", "geo", "market")) {
            return NativeOffloadResult.usage("open: unsupported URI scheme")
        }
        return launchActivity(context, Intent(Intent.ACTION_VIEW, uri), "open")
    }
}

private class PhotosPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult =
        when (request.arguments.firstOrNull()) {
            null, "list" -> list(request.arguments.drop(if (request.arguments.isEmpty()) 0 else 1))
            "open" -> {
                val id = request.arguments.getOrNull(1)?.toLongOrNull()
                    ?: return NativeOffloadResult.usage("photos open <media-id>")
                val uri = ContentUris.withAppendedId(
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                    id,
                )
                launchActivity(context, Intent(Intent.ACTION_VIEW, uri).setType("image/*"), "photos")
            }
            else -> NativeOffloadResult.usage("Usage: photos [list] [--limit <n>] | photos open <media-id>")
        }

    private suspend fun list(raw: List<String>): NativeOffloadResult = withContext(Dispatchers.IO) {
        val args = CommandArguments.parse(raw)
            ?: return@withContext NativeOffloadResult.usage("photos list [--limit <n>]")
        val limit = args.option("limit")?.toIntOrNull()?.coerceIn(1, 100) ?: 30
        val rows = mutableListOf<String>()
        context.contentResolver.query(
            MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
            arrayOf(
                MediaStore.Images.Media._ID,
                MediaStore.Images.Media.DISPLAY_NAME,
                MediaStore.Images.Media.DATE_TAKEN,
                MediaStore.Images.Media.MIME_TYPE,
            ),
            null,
            null,
            "${MediaStore.Images.Media.DATE_TAKEN} DESC",
        )?.use { cursor ->
            val id = cursor.getColumnIndexOrThrow(MediaStore.Images.Media._ID)
            val name = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DISPLAY_NAME)
            val date = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.DATE_TAKEN)
            val mime = cursor.getColumnIndexOrThrow(MediaStore.Images.Media.MIME_TYPE)
            while (cursor.moveToNext() && rows.size < limit) {
                rows += "${cursor.getLong(id)}\t${clean(cursor.getString(name))}\t" +
                    "${cursor.getLong(date)}\t${clean(cursor.getString(mime))}"
            }
        }
        NativeOffloadResult.success(rows.joinToString("\n", postfix = if (rows.isEmpty()) "" else "\n"))
    }
}

internal class PlayerPort(private val audio: OffloadAudioService) : NativeCommandPort {
    private val cleanupScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private data class MediaKey(val owner: AudioOwnerKey, val label: String)

    private val lifetime = OffloadPortLifetime()
    private val namespace = UUID.randomUUID().toString().replace("-", "")
    private val stateLock = Any()
    private val ownedMedia = mutableSetOf<MediaKey>()
    private var closed = false

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult = lifetime.execute {
        if (isClosed()) return@execute NativeOffloadResult.unavailable("player", "audio host is closing")
        val owner = owner(request) ?: return@execute NativeOffloadResult.usage("player: session owner is required")
        val command = request.arguments.firstOrNull()
        val userLabel = when (command) {
            "play" -> request.arguments.getOrNull(2)
            "pause", "resume", "stop", "status" -> request.arguments.getOrNull(1)
            else -> null
        }?.takeIf { it.isNotBlank() } ?: request.sessionId
        val label = serviceLabel(userLabel)

        return@execute when (command) {
            "play" -> {
                val target = request.arguments.getOrNull(1)
                    ?: return@execute NativeOffloadResult.usage("player play <path-or-uri> [session]")
                if (!rememberMedia(owner, userLabel)) {
                    return@execute NativeOffloadResult.unavailable("player", "audio host is closing")
                }
                audio.playMedia(owner, label, target).toPlayerResult(userLabel)
            }
            "pause" -> audio.controlMedia(owner, label, OffloadMediaCommand.PAUSE).toPlayerResult()
            "resume" -> audio.controlMedia(owner, label, OffloadMediaCommand.RESUME).toPlayerResult()
            "stop" -> audio.controlMedia(owner, label, OffloadMediaCommand.STOP).toPlayerResult()
            "status" -> audio.controlMedia(owner, label, OffloadMediaCommand.STATUS).toPlayerResult(includeStatus = true)
            else -> NativeOffloadResult.usage(
                "Usage: player play <path-or-uri> [session] | player pause|resume|stop|status [session]",
            )
        }
    }

    override fun close() {
        val mediaAtClose = synchronized(stateLock) {
            if (closed) return
            closed = true
            ownedMedia.toList()
        }
        lifetime.close()
        cleanupScope.launch {
            mediaAtClose.forEach { media ->
                runCatching {
                    audio.controlMedia(media.owner, serviceLabel(media.label), OffloadMediaCommand.STOP)
                }
            }
        }
    }

    private fun isClosed(): Boolean = synchronized(stateLock) { closed }

    private fun rememberMedia(owner: AudioOwnerKey, label: String): Boolean = synchronized(stateLock) {
        if (closed) {
            false
        } else {
            ownedMedia.add(MediaKey(owner, label))
            true
        }
    }

    private fun serviceLabel(userLabel: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(userLabel.toByteArray(Charsets.UTF_8))
        val hash = digest.joinToString(separator = "") { byte -> (byte.toInt() and 0xff).toString(16).padStart(2, '0') }
        return "$namespace-$hash"
    }

    private fun owner(request: NativeOffloadRequest) = request.sessionId
        .takeIf { it.isNotBlank() }
        ?.let(AudioOwnerKey::session)

}

internal class SpeechPort(private val audio: OffloadAudioService) : NativeCommandPort {
    private val lifetime = OffloadPortLifetime()
    private val closed = AtomicBoolean(false)

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult = lifetime.execute {
        if (closed.get()) return@execute NativeOffloadResult.unavailable("speech", "audio host is closing")
        if (request.arguments.firstOrNull() !in setOf("speak", "say")) {
            return@execute NativeOffloadResult.usage("speech speak <text>")
        }
        val text = request.arguments.drop(1).joinToString(" ")
            .ifEmpty { request.stdin.toString(Charsets.UTF_8) }
        if (text.isBlank()) return@execute NativeOffloadResult.usage("speech speak <text>")
        val owner = request.sessionId.takeIf { it.isNotBlank() }
            ?.let(AudioOwnerKey::session)
            ?: return@execute NativeOffloadResult.usage("speech: session owner is required")
        audio.speak(owner, text, timeoutBudgetMs = SPEECH_TIMEOUT_MS).toSpeechResult()
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        lifetime.close()
    }

    private companion object { const val SPEECH_TIMEOUT_MS = 30_000L }
}

private fun DeviceAudioResult.toPlayerResult(
    label: String? = null,
    includeStatus: Boolean = false,
): NativeOffloadResult = when (this) {
    is DeviceAudioResult.OffloadMedia -> {
        if (includeStatus) {
            NativeOffloadResult.success(
                "playing=${state.playing}\npositionMs=${state.positionMs}\ndurationMs=${state.durationMs}\n",
            )
        } else if (label != null) {
            NativeOffloadResult.success("session=$label\ndurationMs=${state.durationMs}\n")
        } else {
            NativeOffloadResult.success()
        }
    }
    is DeviceAudioResult.Failed -> NativeOffloadResult(
        exitCode = NativeOffloadResult.EXIT_FAILURE,
        stderr = "player: ${error.message}\n".toByteArray(),
    )
    else -> NativeOffloadResult.internal("player", "audio service returned an unexpected result")
}

private fun DeviceAudioResult.toSpeechResult(): NativeOffloadResult = when (this) {
    is DeviceAudioResult.PlaybackCompleted -> NativeOffloadResult.success()
    is DeviceAudioResult.Failed -> NativeOffloadResult(
        exitCode = NativeOffloadResult.EXIT_FAILURE,
        stderr = "speech: ${error.message}\n".toByteArray(),
    )
    else -> NativeOffloadResult.internal("speech", "audio service returned an unexpected result")
}

private class ScheduledPort(private val context: Context) : NativeCommandPort {
    private val repository by lazy { AndroidCronRepository.get(context) }

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult =
        when (request.arguments.firstOrNull()) {
            "list" -> list()
            "create" -> create(request.arguments.drop(1))
            "delete" -> {
                val taskId = request.arguments.getOrNull(1)
                    ?: return NativeOffloadResult.usage("scheduled delete <task-id> [scope-id]")
                val scope = request.arguments.getOrNull(2) ?: "global"
                NativeOffloadResult.success("deleted=${repository.delete(scope, taskId)}\n")
            }
            "run" -> {
                val taskId = request.arguments.getOrNull(1)
                    ?: return NativeOffloadResult.usage("scheduled run <task-id> [scope-id]")
                val scope = request.arguments.getOrNull(2) ?: "global"
                val record = repository.runNow(scope, taskId)
                NativeOffloadResult.success("runId=${record.runId}\nstatus=${record.status}\n")
            }
            else -> NativeOffloadResult.usage(
                "Usage: scheduled list | scheduled create --cron <expr> --prompt <text> [--scope global] | scheduled delete|run <id> [scope]",
            )
        }

    private suspend fun list(): NativeOffloadResult {
        repository.refresh()
        val state = withTimeout(15_000) { repository.state.first { !it.loading } }
        state.errorMessage?.let {
            return NativeOffloadResult(
                exitCode = NativeOffloadResult.EXIT_FAILURE,
                stderr = "scheduled: $it\n".toByteArray(),
            )
        }
        val output = state.tasks.joinToString("\n", postfix = if (state.tasks.isEmpty()) "" else "\n") {
            "${it.scope.scopeId}\t${it.task.id}\t${it.task.cron}\t${clean(it.task.prompt)}\t${it.task.nextFireMs ?: ""}"
        }
        return NativeOffloadResult.success(output)
    }

    private suspend fun create(raw: List<String>): NativeOffloadResult {
        val args = CommandArguments.parse(raw)
            ?: return NativeOffloadResult.usage("scheduled create --cron <expr> --prompt <text>")
        val cron = args.option("cron")
            ?: return NativeOffloadResult.usage("scheduled: --cron is required")
        val prompt = args.option("prompt")
            ?: return NativeOffloadResult.usage("scheduled: --prompt is required")
        val task = repository.create(
            scopeId = args.option("scope") ?: "global",
            cron = cron,
            prompt = prompt,
            recurring = args.option("recurring")?.toBooleanStrictOrNull() ?: true,
        )
        return NativeOffloadResult.success("id=${task.id}\nnextFireMs=${task.nextFireMs ?: ""}\n")
    }
}

private data class CommandArguments(
    private val options: Map<String, String>,
) {
    fun option(name: String): String? = options[name]

    companion object {
        fun parse(raw: List<String>): CommandArguments? {
            val options = linkedMapOf<String, String>()
            var index = 0
            while (index < raw.size) {
                val token = raw[index]
                if (!token.startsWith("--") || index + 1 >= raw.size) return null
                options[token.removePrefix("--")] = raw[index + 1]
                index += 2
            }
            return CommandArguments(options)
        }
    }
}

private fun launchActivity(context: Context, intent: Intent, tool: String): NativeOffloadResult {
    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    if (intent.resolveActivity(context.packageManager) == null) {
        return NativeOffloadResult.unavailable(tool, "no Android activity can handle this action")
    }
    context.startActivity(intent)
    return NativeOffloadResult.success("launched=true\n")
}

private fun clean(value: String?): String =
    value.orEmpty().replace('\t', ' ').replace('\n', ' ').replace('\r', ' ')
