package com.lingxi.code.teams

import android.content.Context
import android.webkit.WebView
import android.webkit.WebViewClient
import android.webkit.WebResourceRequest
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedCard
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.lingxi.code.R
import java.util.Locale
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONObject

internal data class TeamVisualizationReference(val page: String, val version: String, val title: String, val bindings: Map<String, String>) {
    companion object {
        fun parse(value: JSONObject?, id: String): TeamVisualizationReference? {
            if (value == null || !id.startsWith("viz_")) return null
            val ref = value.optJSONObject("reference")?.let(DatasetResourceRef::parse) as? DatasetResourceRef.PageVersion ?: return null
            if (ref.page != id) return null
            val title = value.optString("title").trim().takeIf { it.isNotEmpty() && it.length <= 80 } ?: return null
            val items = value.optJSONArray("bindings") ?: return null
            if (items.length() > 4) return null
            val bindings = linkedMapOf<String, String>()
            for (index in 0 until items.length()) {
                val item = items.optJSONObject(index) ?: return null
                val alias = item.optString("alias")
                val mode = item.optString("mode")
                if (!alias.matches(Regex("[A-Za-z0-9_-]{1,64}")) || mode !in setOf("pinned", "live", "edit") || alias in bindings) return null
                bindings[alias] = mode
            }
            return TeamVisualizationReference(ref.page, ref.version, title, bindings)
        }
    }
}

private const val surfaceOrigin = "https://team-visualization.invalid"
private const val bridgeName = "teamVisualization"
private const val surfacePath = "team-visualization.html"
private val authorizationErrors = setOf("DATA_ACCESS_REQUIRED", "STALE_MEMBERSHIP", "INVALID_SESSION", "SIGN_IN_REQUIRED", "DATA_NOT_FOUND")

internal fun objectOf(vararg entries: Pair<String, Any?>): JSONObject = JSONObject().also { json ->
    entries.forEach { (key, value) -> if (value != null) json.put(key, value) }
}

internal fun surfacePacket(type: String, data: Any? = null): JSONObject = objectOf("type" to type, "value" to data)

internal fun surfaceLoad(bundle: JSONObject, dark: Boolean, aliases: Collection<String> = emptyList(), state: Any? = null) =
    objectOf("bundle" to bundle, "aliases" to org.json.JSONArray(aliases), "locale" to Locale.getDefault().toLanguageTag(),
        "theme" to objectOf("mode" to if (dark) "dark" else "light", "tokens" to JSONObject()), "state" to state)

private fun webView(context: Context) = WebView(context).apply {
    settings.javaScriptEnabled = true
    settings.domStorageEnabled = false
    settings.allowFileAccess = false
    settings.allowContentAccess = false
    settings.blockNetworkLoads = true
    settings.javaScriptCanOpenWindowsAutomatically = false
    settings.setSupportMultipleWindows(false)
    webViewClient = object : WebViewClient() {
        override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true
    }
    setBackgroundColor(android.graphics.Color.TRANSPARENT)
}

@Composable
internal fun TeamVisualizationSurface(load: JSONObject, height: Int, connected: ((JSONObject) -> Unit) -> Unit, handle: (JSONObject) -> Unit) {
    val currentHandle by rememberUpdatedState(handle)
    val currentConnect by rememberUpdatedState(connected)
    if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
        Text(stringResource(R.string.teams_data_unavailable))
        return
    }
    AndroidView(
        modifier = Modifier.fillMaxWidth().height(height.dp),
        factory = { context ->
            webView(context).apply {
                var ready = false
                WebViewCompat.addWebMessageListener(this, bridgeName, setOf(surfaceOrigin)) { _, message, origin, mainFrame, reply ->
                    if (!mainFrame || origin.toString().trimEnd('/') != surfaceOrigin) return@addWebMessageListener
                    val input = message.data.orEmpty()
                    if (input.length > 131_072) return@addWebMessageListener
                    val frame = runCatching { JSONObject(input) }.getOrNull() ?: return@addWebMessageListener
                    if (!ready && frame.optString("type") == "ready") {
                        ready = true
                        send(reply, surfacePacket("initialize", load))
                        currentConnect { send(reply, it) }
                    } else if (ready) {
                        currentHandle(frame)
                    }
                }
                val html = context.assets.open(surfacePath).bufferedReader().use { it.readText() }
                loadDataWithBaseURL("$surfaceOrigin/$surfacePath", html, "text/html", "utf-8", null)
            }
        },
        onRelease = { view ->
            WebViewCompat.removeWebMessageListener(view, bridgeName)
            view.stopLoading()
            view.destroy()
        },
    )
}

private fun send(reply: JavaScriptReplyProxy, packet: JSONObject) {
    runCatching { reply.postMessage(packet.toString()) }
}

private data class PatchProposal(val alias: String, val record: String, val previous: JSONObject, val updates: JSONObject,
    val conflict: Boolean, val answer: CompletableDeferred<Boolean>)

@Composable
internal fun TeamVisualizationCard(reference: TeamVisualizationReference, conversation: String, model: TeamMessagingModel,
    online: Boolean, destinations: List<TeamMessageConversation>, person: (String) -> String) {
    val dark = isSystemInDarkTheme()
    val coroutines = rememberCoroutineScope()
    var opened by remember(reference) { mutableStateOf<JSONObject?>(null) }
    var height by remember(reference) { mutableIntStateOf(44) }
    var attempt by remember(reference) { mutableIntStateOf(0) }
    var failed by remember(reference) { mutableStateOf(false) }
    var expanded by remember(reference) { mutableStateOf(false) }
    var forwarding by remember(reference) { mutableStateOf(false) }
    var forwardTarget by remember(reference) { mutableStateOf<TeamMessageConversation?>(null) }
    var forwardAudience by remember(reference) { mutableStateOf<JSONObject?>(null) }
    var forwardFailed by remember(reference) { mutableStateOf(false) }
    var emit by remember(reference) { mutableStateOf<((JSONObject) -> Unit)?>(null) }
    var proposal by remember(reference) { mutableStateOf<PatchProposal?>(null) }
    var submitting by remember(reference) { mutableStateOf(false) }
    val watches = remember(reference) { linkedMapOf<String, JSONObject>() }

    fun approve(value: Boolean) {
        proposal?.answer?.complete(value)
        proposal = null
    }
    suspend fun request(action: String, fields: JSONObject = JSONObject()): JSONObject =
        model.visualization(conversation, fields.put("action", action).put("message", reference.page).put("version", reference.version))
    suspend fun data(alias: String, action: String, fields: JSONObject = JSONObject(), operation: String? = null): JSONObject =
        request("data", objectOf("alias" to alias, "request" to fields.put("action", action), "operation" to operation))

    suspend fun approvePatch(alias: String, raw: JSONObject): JSONObject {
        val record = raw.getString("record")
        val changes = raw.getJSONObject("changes")
        if (reference.bindings[alias] != "edit" || submitting || changes.length() == 0) throw TeamMessageFailure("DATA_ACCESS_REQUIRED")
        submitting = true
        try {
            repeat(2) { retry ->
                val now = data(alias, "read", objectOf("record" to record))
                val row = now.getJSONObject("data").getJSONArray("records").getJSONObject(0)
                if (opened == null || now.optString("access") != "edit" || row.optBoolean("archived")) throw TeamMessageFailure("DATA_ACCESS_REQUIRED")
                val answer = CompletableDeferred<Boolean>()
                proposal = PatchProposal(alias, record, row.getJSONObject("value"), changes, retry != 0, answer)
                if (!answer.await() || opened == null) throw TeamMessageFailure("DATA_CANCELLED")
                try {
                    return data(alias, "patch", objectOf("record" to record, "changes" to changes,
                        "schemaVersion" to now.getLong("schemaVersion"), "expectedRevision" to row.getJSONObject("reference").getLong("revision"),
                        "partition" to row.getString("partition")), UUID.randomUUID().toString())
                } catch (error: TeamMessageFailure) {
                    if (error.code != "DATA_CONFLICT" || retry != 0) throw error
                }
            }
            throw TeamMessageFailure("DATA_CONFLICT")
        } finally { submitting = false }
    }

    suspend fun handle(packet: JSONObject): Any {
        if (opened == null) throw TeamMessageFailure("DATA_ACCESS_REQUIRED")
        val action = packet.getString("action")
        if (action == "expand") { expanded = true; return JSONObject.NULL }
        if (action == "state") return request("state", objectOf("expected" to packet.getLong("expected"), "state" to packet.getJSONObject("state")))
        val alias = packet.getString("alias")
        if (alias !in reference.bindings) throw TeamMessageFailure("DATA_ACCESS_REQUIRED")
        return when (action) {
            "read" -> data(alias, "read", objectOf("after" to packet.stringOrNull("after"),
                "block" to packet.opt("block")?.takeUnless { it == JSONObject.NULL })).get("data")
            "watch" -> {
                val snapshot = watches[alias] ?: data(alias, "read", objectOf("watch" to true))
                watches[alias] = snapshot
                snapshot.get("data")
            }
            "patch" -> approvePatch(alias, packet.getJSONObject("changes"))
            else -> throw TeamMessageFailure("INVALID_DATA_REQUEST")
        }
    }

    LaunchedEffect(reference, online, attempt) {
        failed = false
        if (!online) { opened = null; approve(false); return@LaunchedEffect }
        try {
            opened = request("open")
            while (currentCoroutineContext().isActive) {
                delay(3000)
                request("check")
                for ((alias, last) in watches.toList()) {
                    val cursors = last.optJSONArray("cursors")
                    if (cursors != null) {
                        val changes = try { data(alias, "changes", objectOf("cursors" to cursors)) }
                        catch (error: TeamMessageFailure) {
                            if (error.code !in setOf("DATA_BACKFILL_REQUIRED", "INVALID_DATA_REQUEST")) throw error
                            null
                        }
                        if (changes != null && !changes.getBoolean("changed")
                            && changes.optLong("schemaVersion") == last.optLong("schemaVersion")) {
                            last.put("cursors", changes.getJSONArray("cursors"))
                            continue
                        }
                    }
                    val fresh = data(alias, "read", objectOf("watch" to true))
                    if (fresh.get("data").toString() != last.get("data").toString()) {
                        emit?.invoke(surfacePacket("data", objectOf("alias" to alias, "value" to fresh.get("data"))))
                    }
                    watches[alias] = fresh
                }
            }
        } catch (error: CancellationException) { throw error }
        catch (_: Exception) { failed = true }
        finally {
            opened = null
            emit?.invoke(surfacePacket("suspend"))
            emit = null
            watches.clear()
            approve(false)
        }
    }
    LaunchedEffect(expanded, dark) {
        emit?.invoke(surfacePacket("layout", objectOf("expanded" to expanded, "theme" to objectOf(
            "mode" to if (dark) "dark" else "light", "tokens" to JSONObject()))))
    }
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(10.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(reference.title, style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
                if (opened != null && destinations.isNotEmpty()) TextButton(onClick = { forwarding = true }) {
                    Text(stringResource(R.string.teams_visualization_forward))
                }
                if (opened != null) TextButton(onClick = { expanded = !expanded }) {
                    Text(stringResource(if (expanded) R.string.teams_visualization_collapse else R.string.teams_visualization_expand))
                }
            }
            if (failed) {
                Text(stringResource(R.string.teams_data_unavailable))
                TextButton(onClick = { attempt++ }) { Text(stringResource(R.string.teams_visualization_retry)) }
            }
            if (!failed) opened?.let { value ->
                val load = remember(value) { surfaceLoad(value.getJSONObject("bundle"), dark, reference.bindings.keys, value.opt("state")) }
                TeamVisualizationSurface(load, height.coerceIn(32, if (expanded) 880 else 520), { emit = it }) { packet ->
                    when (packet.optString("type")) {
                        "height" -> height = packet.optInt("height", 44)
                        "error" -> failed = true
                        "request" -> {
                            val id = packet.optLong("id", -1)
                            if (id >= 0) coroutines.launch {
                                val result = try { objectOf("id" to id, "value" to handle(packet)) }
                                catch (cancel: CancellationException) { throw cancel }
                                catch (error: Exception) {
                                    val code = (error as? TeamMessageFailure)?.code ?: "DATA_UNAVAILABLE"
                                    if (code in authorizationErrors) { failed = true; opened = null }
                                    objectOf("id" to id, "error" to code)
                                }
                                emit?.invoke(surfacePacket("result", result))
                            }
                        }
                    }
                }
            }
        }
    }
    val bundle = opened?.optJSONObject("bundle")
    if (forwarding && bundle != null) {
        val selected = forwardTarget
        val audience = forwardAudience
        if (selected != null && audience != null) {
            TeamVisualizationComposer(selected.id, model, person, audience, source = bundle, close = {
                forwarding = false; forwardTarget = null; forwardAudience = null
            })
        } else {
            AlertDialog(onDismissRequest = { forwarding = false; forwardTarget = null },
                title = { Text(stringResource(R.string.teams_visualization_destination)) },
                text = { Column(Modifier.verticalScroll(rememberScrollState())) {
                    destinations.forEach { target ->
                        TextButton(onClick = { forwardFailed = false; forwardTarget = target }) { Text(target.title) }
                    }
                    if (forwardFailed) Text(stringResource(R.string.teams_data_unavailable))
                } },
                confirmButton = {})
            LaunchedEffect(selected) {
                if (selected != null) try {
                    forwardAudience = model.visualization(selected.id, objectOf("action" to "audience"))
                } catch (cancel: CancellationException) { throw cancel }
                catch (_: Exception) { forwardFailed = true; forwardTarget = null }
            }
        }
    }
    proposal?.let { edit ->
        AlertDialog(
            onDismissRequest = { approve(false) },
            title = { Text(stringResource(R.string.teams_visualization_confirm)) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    if (edit.conflict) Text(stringResource(R.string.teams_visualization_conflict))
                    Text(edit.alias + " · " + edit.record)
                    edit.updates.keys().forEach { field ->
                        Text(field + "  " + (edit.previous.opt(field) ?: "—") + " → " + edit.updates.get(field))
                    }
                }
            },
            confirmButton = { TextButton(onClick = { approve(true) }) { Text(stringResource(R.string.teams_visualization_apply)) } },
            dismissButton = { TextButton(onClick = { approve(false) }) { Text(stringResource(android.R.string.cancel)) } },
        )
    }
}
