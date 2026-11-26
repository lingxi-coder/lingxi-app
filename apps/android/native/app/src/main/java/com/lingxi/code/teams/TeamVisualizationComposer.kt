package com.lingxi.code.teams

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import java.io.ByteArrayOutputStream
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

private const val packageLimit = 2 * 1024 * 1024
private val wireAlias = Regex("[A-Za-z0-9_-]{1,64}")

@Composable
internal fun TeamVisualizationComposerButton(conversation: String, model: TeamMessagingModel, person: (String) -> String) {
    var audience by remember(conversation) { mutableStateOf<JSONObject?>(null) }
    var open by remember(conversation) { mutableStateOf(false) }
    LaunchedEffect(conversation, model) {
        audience = try { model.visualization(conversation, objectOf("action" to "audience")) }
        catch (cancel: CancellationException) { throw cancel }
        catch (_: Exception) { null }
    }
    val confirmation = audience ?: return
    TextButton(onClick = { open = true }) { Text(stringResource(R.string.teams_visualization_add)) }
    if (open) TeamVisualizationComposer(conversation, model, person, confirmation, close = { open = false })
}

@Composable
internal fun TeamVisualizationComposer(conversation: String, model: TeamMessagingModel, person: (String) -> String,
    initialAudience: JSONObject, source: JSONObject? = null, from: String = conversation, close: () -> Unit,
) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val dark = isSystemInDarkTheme()
    var audience by remember { mutableStateOf(initialAudience) }
    var title by remember { mutableStateOf(source?.optString("title").orEmpty()) }
    var html by remember { mutableStateOf(source?.optString("html").orEmpty()) }
    var preview by remember { mutableStateOf<JSONObject?>(null) }
    var height by remember { mutableIntStateOf(44) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var building by remember { mutableStateOf(false) }
    var intent by remember { mutableStateOf<Pair<String, String>?>(null) }
    var location by remember { mutableStateOf("mine") }
    var choices by remember { mutableStateOf(emptyList<JSONObject>()) }
    var cursor by remember { mutableStateOf<String?>(null) }
    var discoveryId by remember { mutableStateOf(UUID.randomUUID()) }
    var discovering by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf<JSONObject?>(null) }
    var alias by remember { mutableStateOf("") }
    var mode by remember { mutableStateOf("pinned") }
    var fields by remember { mutableStateOf("") }
    var partitions by remember { mutableStateOf("") }
    var bindings by remember { mutableStateOf(emptyList<JSONObject>()) }
    val valid = title.isNotBlank() && title.trim().length <= 80 && html.isNotBlank()
    val bundle = { objectOf("title" to title.trim(), "html" to html, "assets" to (source?.optJSONObject("assets") ?: JSONObject())) }
    val translator = mapOf("pinned" to stringResource(R.string.teams_dataset_open_fixed),
        "live" to stringResource(R.string.teams_dataset_open_live), "edit" to stringResource(R.string.teams_visualization_edit))
    val openFile = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            try {
                html = withContext(Dispatchers.IO) {
                    context.contentResolver.openInputStream(uri)?.use { stream ->
                        val contents = ByteArrayOutputStream()
                        val buffer = ByteArray(32 * 1024)
                        while (true) {
                            val count = stream.read(buffer)
                            if (count < 0) break
                            if (contents.size() + count > packageLimit) throw TeamMessageFailure("VISUALIZATION_TOO_LARGE")
                            contents.write(buffer, 0, count)
                        }
                        contents.toString(Charsets.UTF_8.name())
                    } ?: throw TeamMessageFailure("INVALID_DATA")
                }
                if (title.isBlank()) title = uri.lastPathSegment?.substringAfterLast('/')?.substringBeforeLast('.')?.take(80).orEmpty()
                preview = null
            } catch (cancel: CancellationException) { throw cancel }
            catch (failure: Exception) { error = (failure as? TeamMessageFailure)?.code ?: "INVALID_DATA" }
        }
    }

    suspend fun discover(after: String? = null) {
        val request = UUID.randomUUID()
        val source = location
        discoveryId = request
        discovering = true
        if (after == null) { selected = null; choices = emptyList(); cursor = null }
        val (destination, _) = model.datasetShareDestination(conversation)
        val filter = when (source) {
            "shared" -> objectOf("kind" to "conversation", "destination" to destination)
            "team" -> objectOf("kind" to "mine", "team" to destination.getString("team"))
            else -> objectOf("kind" to "mine")
        }
        try {
            val result = model.dataset(conversation, objectOf("protocol" to DatasetResourceRef.PROTOCOL_VERSION,
                "command" to objectOf("action" to "discover", "scope" to filter, "limit" to 50, "after" to after)), from = from)
            if (discoveryId != request || location != source) return
            val entries = result.getJSONArray("resources")
            val found = (0 until entries.length()).map(entries::getJSONObject).filter { item ->
                if (source == "shared") item.stringOrNull("via") != null else item.optBoolean("canShare")
            }
            choices = if (after == null) found else choices + found
            cursor = result.stringOrNull("cursor")
        } catch (cancel: CancellationException) { throw cancel }
        catch (_: Exception) { /* Standalone HTML needs no dataset. */ }
        finally { if (discoveryId == request) discovering = false }
    }

    LaunchedEffect(location, conversation) { discover() }

    fun add() {
        val item = selected ?: return
        val reference = item.getJSONObject("reference")
        val source = objectOf("alias" to alias, "mode" to mode)
        if (location == "shared") {
            source.put("kind", "reuse").put("grant", item.getString("via")).put("reference", reference)
        } else {
            val projection = JSONObject()
            listOf("fields" to fields, "partitions" to partitions).forEach { (key, text) ->
                val values = text.split(',').map(String::trim).filter(String::isNotEmpty).distinct()
                if (values.isNotEmpty()) projection.put(key, JSONArray(values))
            }
            source.put("kind", "share").put("dataset", reference.getString("dataset")).put("projection", projection)
            if (reference.optString("kind") != "dataset") source.put("reference", reference)
        }
        bindings = bindings + source
        selected = null
    }
    fun publish() {
        if (busy || !valid || selected != null) return
        val content = bundle()
        if (content.toString().toByteArray().size > packageLimit) { error = "VISUALIZATION_TOO_LARGE"; return }
        val body = objectOf("action" to "publish", "bundle" to content, "bindings" to JSONArray(bindings),
            "audienceDigest" to audience.getString("audienceDigest"))
        val draft = body.toString()
        if (intent?.first != draft) intent = draft to UUID.randomUUID().toString()
        body.put("operation", intent?.second)
        busy = true
        scope.launch {
            error = null
            try {
                val result = model.visualization(conversation, body, from = from)
                if (result.optString("status") == "building") building = true else close()
            } catch (cancel: CancellationException) { throw cancel }
            catch (failure: Exception) {
                error = (failure as? TeamMessageFailure)?.code ?: "VISUALIZATION_UNAVAILABLE"
                if (error == "DATA_SHARE_CHANGED") {
                    intent = null
                    building = false
                    try { audience = model.visualization(conversation, objectOf("action" to "audience"), from = from) }
                    catch (cancel: CancellationException) { throw cancel }
                    catch (_: Exception) { /* Keep the send error visible. */ }
                }
            } finally { busy = false }
        }
    }

    AlertDialog(
        onDismissRequest = { if (!busy) close() },
        title = { Text(stringResource(R.string.teams_visualization_add)) },
        text = {
            Column(Modifier.fillMaxWidth().heightIn(max = 580.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(title, { title = it.take(80); preview = null }, label = { Text(stringResource(R.string.teams_visualization_title)) })
                TextButton(onClick = { openFile.launch(arrayOf("text/html", "text/plain")) }) {
                    Text(stringResource(R.string.teams_visualization_import))
                }
                OutlinedTextField(html, { html = it; preview = null }, label = { Text("HTML") }, minLines = 4)
                TextButton(enabled = valid, onClick = { preview = bundle() }) {
                    Text(stringResource(R.string.teams_visualization_preview))
                }
                preview?.let { content ->
                    val aliases = bindings.map { it.getString("alias") }
                    key(content, aliases, dark) {
                        var send by remember { mutableStateOf<((JSONObject) -> Unit)?>(null) }
                        TeamVisualizationSurface(surfaceLoad(content, dark, aliases), height.coerceIn(32, 360), { send = it }) { packet ->
                            when (packet.optString("type")) {
                                "height" -> height = packet.optInt("height", 44)
                                "error" -> error = "VISUALIZATION_SCRIPT_FAILED"
                                "request" -> send?.invoke(surfacePacket("result", previewResult(packet, aliases)))
                            }
                        }
                    }
                }
                val locations = mapOf("mine" to stringResource(R.string.teams_visualization_personal),
                    "team" to stringResource(R.string.teams_visualization_team), "shared" to stringResource(R.string.teams_visualization_shared))
                PickerButton(locations.getValue(location), locations.toList()) { location = it }
                if (choices.isNotEmpty()) PickerButton(selected?.getJSONObject("reference")?.optString("dataset")
                    ?: stringResource(R.string.teams_dataset_title), choices.mapIndexed { index, entry ->
                    index to entry.getJSONObject("reference").getString("dataset")
                }) {
                    val item = choices[it]
                    selected = item
                    alias = item.getJSONObject("reference").getString("dataset")
                    mode = if (location == "shared") item.getString("access") else "pinned"
                    fields = ""; partitions = ""
                }
                cursor?.let { next ->
                    TextButton(onClick = { scope.launch { discover(next) } }, enabled = !discovering) {
                        Text(stringResource(R.string.teams_visualization_more))
                    }
                }
                selected?.let { item ->
                    OutlinedTextField(alias, { alias = it.take(64) }, label = { Text(stringResource(R.string.teams_visualization_alias)) })
                    val access = item.getString("access")
                    val modes = when {
                        location != "shared" && access == "edit" -> listOf("pinned", "live", "edit")
                        access == "edit" -> listOf("live", "edit")
                        else -> listOf(access)
                    }
                    PickerButton(translator.getValue(mode), modes.map { it to translator.getValue(it) }) { mode = it }
                    if (location != "shared") {
                        OutlinedTextField(fields, { fields = it }, label = { Text(stringResource(R.string.teams_visualization_fields)) })
                        OutlinedTextField(partitions, { partitions = it }, label = { Text(stringResource(R.string.teams_visualization_partitions)) })
                    }
                    TextButton(onClick = { add() }, enabled = alias.matches(wireAlias) && bindings.size < 4
                        && bindings.none { it.optString("alias") == alias }) { Text(stringResource(R.string.teams_dataset_title)) }
                }
                for (item in bindings) Row {
                    Text(item.getString("alias") + " · " + translator.getValue(item.getString("mode")))
                    TextButton(onClick = { bindings = bindings.filterNot { it === item } }) { Text(stringResource(R.string.teams_visualization_remove)) }
                }
                Text(stringResource(R.string.teams_visualization_audience_fmt,
                    audience.getJSONObject("audience").keys().asSequence().map(person).joinToString(" · ")))
                if (building) Text(stringResource(R.string.teams_visualization_building))
                error?.let { Text(it) }
            }
        },
        confirmButton = {
            TextButton(onClick = { publish() }, enabled = valid && selected == null && !busy) {
                Text(stringResource(if (building) R.string.teams_visualization_retry else R.string.teams_visualization_send))
            }
        },
        dismissButton = { TextButton(onClick = close, enabled = !busy) { Text(stringResource(android.R.string.cancel)) } },
    )
}

private fun previewResult(packet: JSONObject, aliases: List<String>): JSONObject {
    val result = objectOf("id" to packet.opt("id"))
    val expected = packet.optLong("expected", -1)
    val state = packet.optJSONObject("state")
    return when (packet.optString("action")) {
        "read", "watch" -> if (packet.optString("alias") in aliases) {
            result.put("value", objectOf("records" to JSONArray()))
        } else result.put("error", "DATA_ACCESS_REQUIRED")
        "unwatch" -> result.put("value", JSONObject.NULL)
        "state" -> if (state != null && expected in 0 until Long.MAX_VALUE) {
            result.put("value", state.put("version", expected + 1))
        } else result.put("error", "DATA_ACCESS_REQUIRED")
        else -> result.put("error", "DATA_ACCESS_REQUIRED")
    }
}

@Composable
private fun <T> PickerButton(label: String, choices: List<Pair<T, String>>, choose: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Column {
        TextButton(onClick = { open = true }) { Text(label) }
        DropdownMenu(open, { open = false }) {
            choices.forEach { (value, text) -> DropdownMenuItem(text = { Text(text) }, onClick = { open = false; choose(value) }) }
        }
    }
}
