package com.lingxi.code.offload

import android.Manifest
import android.content.ContentUris
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationManager
import android.media.MediaPlayer
import android.net.Uri
import android.provider.AlarmClock
import android.provider.CalendarContract
import android.provider.ContactsContract
import android.provider.MediaStore
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import androidx.core.content.ContextCompat
import com.lingxi.code.cron.AndroidCronRepository
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/** Real Android-backed command ports available to both distributions. */
internal object AndroidSystemOffloadPorts {
    fun create(context: Context): Map<String, NativeCommandPort> {
        val app = context.applicationContext
        return mapOf(
            "alarm" to AlarmPort(app),
            "calendar" to CalendarPort(app),
            "contacts" to ContactsPort(app),
            "location" to LocationPort(app),
            "open" to OpenPort(app),
            "photos" to PhotosPort(app),
            "player" to PlayerPort(app),
            "speech" to SpeechPort(app),
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

private class PlayerPort(private val context: Context) : NativeCommandPort {
    private val players = ConcurrentHashMap<String, MediaPlayer>()

    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult =
        when (request.arguments.firstOrNull()) {
            "play" -> play(request)
            "pause" -> withPlayer(request) { it.pause(); NativeOffloadResult.success() }
            "resume" -> withPlayer(request) { it.start(); NativeOffloadResult.success() }
            "stop" -> stop(request.arguments.getOrNull(1) ?: request.sessionId)
            "status" -> withPlayer(request) {
                NativeOffloadResult.success(
                    "playing=${it.isPlaying}\npositionMs=${it.currentPosition}\ndurationMs=${it.duration}\n",
                )
            }
            else -> NativeOffloadResult.usage(
                "Usage: player play <path-or-uri> [session] | player pause|resume|stop|status [session]",
            )
        }

    private suspend fun play(request: NativeOffloadRequest): NativeOffloadResult =
        withContext(Dispatchers.IO) {
            val target = request.arguments.getOrNull(1)
                ?: return@withContext NativeOffloadResult.usage("player play <path-or-uri> [session]")
            val session = request.arguments.getOrNull(2) ?: request.sessionId
            stop(session)
            val player = MediaPlayer()
            try {
                if (target.contains("://")) {
                    player.setDataSource(context, Uri.parse(target))
                } else {
                    val file = File(target).canonicalFile
                    if (!file.exists() || !file.isFile) {
                        player.release()
                        return@withContext NativeOffloadResult(
                            exitCode = NativeOffloadResult.EXIT_FAILURE,
                            stderr = "player: file not found\n".toByteArray(),
                        )
                    }
                    player.setDataSource(file.path)
                }
                player.prepare()
                player.start()
                players[session] = player
                player.setOnCompletionListener { stop(session) }
                NativeOffloadResult.success("session=$session\ndurationMs=${player.duration}\n")
            } catch (error: Throwable) {
                player.release()
                throw error
            }
        }

    private fun withPlayer(
        request: NativeOffloadRequest,
        block: (MediaPlayer) -> NativeOffloadResult,
    ): NativeOffloadResult {
        val session = request.arguments.getOrNull(1) ?: request.sessionId
        val player = players[session]
            ?: return NativeOffloadResult(
                exitCode = NativeOffloadResult.EXIT_FAILURE,
                stderr = "player: unknown session '$session'\n".toByteArray(),
            )
        return block(player)
    }

    private fun stop(session: String): NativeOffloadResult {
        val player = players.remove(session) ?: return NativeOffloadResult.success()
        runCatching { if (player.isPlaying) player.stop() }
        player.release()
        return NativeOffloadResult.success()
    }

    override fun close() {
        players.keys.toList().forEach(::stop)
    }
}

private class SpeechPort(private val context: Context) : NativeCommandPort {
    override suspend fun execute(request: NativeOffloadRequest): NativeOffloadResult {
        if (request.arguments.firstOrNull() !in setOf("speak", "say")) {
            return NativeOffloadResult.usage("speech speak <text>")
        }
        val text = request.arguments.drop(1).joinToString(" ")
            .ifEmpty { request.stdin.toString(Charsets.UTF_8) }
        if (text.isBlank()) return NativeOffloadResult.usage("speech speak <text>")
        return withContext(Dispatchers.Main) {
            withTimeout(30_000) {
                val ready = CompletableDeferred<Pair<TextToSpeech, Int>>()
                lateinit var engine: TextToSpeech
                engine = TextToSpeech(context) { status -> ready.complete(engine to status) }
                val (tts, status) = ready.await()
                if (status != TextToSpeech.SUCCESS) {
                    tts.shutdown()
                    return@withTimeout NativeOffloadResult(
                        exitCode = NativeOffloadResult.EXIT_FAILURE,
                        stderr = "speech: Android TTS initialization failed\n".toByteArray(),
                    )
                }
                try {
                    val done = CompletableDeferred<Boolean>()
                    val utteranceId = "lingxi-offload-${UUID.randomUUID()}"
                    tts.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                        override fun onStart(id: String?) = Unit
                        override fun onDone(id: String?) { done.complete(true) }
                        override fun onError(id: String?) { done.complete(false) }
                        override fun onError(id: String?, errorCode: Int) { done.complete(false) }
                    })
                    val queued = tts.speak(text, TextToSpeech.QUEUE_FLUSH, null, utteranceId)
                    if (queued != TextToSpeech.SUCCESS) done.complete(false)
                    val succeeded = done.await()
                    if (succeeded) NativeOffloadResult.success() else NativeOffloadResult(
                        exitCode = NativeOffloadResult.EXIT_FAILURE,
                        stderr = "speech: synthesis/playback failed\n".toByteArray(),
                    )
                } finally {
                    runCatching { tts.stop() }
                    runCatching { tts.shutdown() }
                }
            }
        }
    }
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
