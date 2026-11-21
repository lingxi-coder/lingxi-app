package com.lingxi.code.teams

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

private data class DatasetChoice(val id: String, val title: String, val fields: List<String>, val digest: String, val people: List<String>)

@Composable
internal fun TeamDatasetShareButton(conversation: TeamMessageConversation, model: TeamMessagingModel, person: (String) -> String) {
    var open by remember(conversation.id) { mutableStateOf(false) }
    TextButton(onClick = { open = true }) { Text(stringResource(R.string.teams_dataset_title)) }
    if (open) TeamDatasetShareDialog(conversation, model, person) { open = false }
}

@Composable
private fun TeamDatasetShareDialog(
    conversation: TeamMessageConversation,
    model: TeamMessagingModel,
    person: (String) -> String,
    close: () -> Unit,
) {
    val thread = conversation.id
    val (destination, generation) = remember(conversation.id) { model.datasetShareDestination(thread) }
    val scope = rememberCoroutineScope()
    var available by remember { mutableStateOf<List<String>>(emptyList()) }
    var cursor by remember { mutableStateOf<String?>(null) }
    var choice by remember { mutableStateOf<DatasetChoice?>(null) }
    var fields by remember { mutableStateOf<Set<String>>(emptySet()) }
    var mode by remember { mutableStateOf("pinned") }
    var busy by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var submission by remember { mutableStateOf<JSONObject?>(null) }

    suspend fun run(operation: suspend () -> Unit) {
        busy = true
        failed = false
        try { operation() }
        catch (cancel: CancellationException) { throw cancel }
        catch (_: Exception) { failed = true }
        finally { busy = false }
    }

    suspend fun data(dataset: String, command: JSONObject): JSONObject =
        model.dataset(thread, JSONObject().put("protocol", DatasetResourceRef.PROTOCOL_VERSION)
            .put("dataset", dataset).put("command", command))

    suspend fun discover(after: String?) = run {
        val rows = data("", JSONObject().put("action", "discover").put("scope", JSONObject().put("kind", "mine"))
            .apply { if (after != null) put("after", after) })
        val array = rows.getJSONArray("resources")
        val next = (0 until array.length()).mapNotNull { index ->
            val row = array.getJSONObject(index)
            if (!row.optBoolean("canShare") || !row.isNull("via")) null
            else (DatasetResourceRef.parse(row.getJSONObject("reference")) as? DatasetResourceRef.Dataset)?.dataset
        }
        available = if (after == null) next else (available + next).distinct()
        cursor = rows.opt("cursor") as? String
    }

    suspend fun select(dataset: String) = run {
        val description = data(dataset, JSONObject().put("action", "describe")).getJSONObject("schema")
        val propertyNames = description.getJSONObject("properties").keys().asSequence().sorted().toList()
        if (propertyNames.isEmpty()) throw TeamMessageFailure("INVALID_DATA")
        val audience = data(dataset, JSONObject().put("action", "audience").put("destination", destination))
        choice = DatasetChoice(dataset, description.optString("title", dataset), propertyNames, audience.getString("audienceDigest"),
            audience.getJSONObject("audience").keys().asSequence().sorted().toList())
        fields = propertyNames.toSet()
        mode = "pinned"
    }

    suspend fun publish(selected: DatasetChoice) = run {
        val body = submission ?: JSONObject().put("action", "share").put("dataset", selected.id)
            .put("destination", destination).put("generation", generation).put("audienceDigest", selected.digest)
            .put("operation", UUID.randomUUID().toString()).put("text", selected.title).put("mode", mode)
            .put("projection", JSONObject().put("fields", JSONArray(fields.sorted()))).apply {
                if (conversation.kind == "bot_dm") put("sharedBot", conversation.sharedBotId ?: throw TeamMessageFailure("INVALID_DATA"))
            }.also { submission = it }
        if (model.dataset(thread, body).getString("status") == "published") close()
    }

    LaunchedEffect(thread) { discover(null) }
    AlertDialog(
        onDismissRequest = { if (!busy) close() },
        title = { Text(stringResource(R.string.teams_dataset_title)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                if (failed) Text(stringResource(R.string.teams_data_unavailable))
                val selected = choice
                if (selected == null) {
                    available.forEach { dataset ->
                        TextButton(enabled = !busy, onClick = { scope.launch { select(dataset) } }) { Text(dataset) }
                    }
                    cursor?.let { after ->
                        TextButton(enabled = !busy, onClick = { scope.launch { discover(after) } }) { Text(stringResource(R.string.teams_dataset_next)) }
                    }
                    if (!busy && available.isEmpty() && !failed) Text(stringResource(R.string.teams_dataset_no_rows))
                    if (failed && available.isEmpty()) TextButton(onClick = { scope.launch { discover(null) } }) { Text(stringResource(R.string.common_retry)) }
                } else {
                    Text(selected.title)
                    Text(stringResource(R.string.teams_dataset_audience_fmt, selected.people.joinToString(" · ", transform = person)))
                    val modes = listOf("pinned" to R.string.teams_dataset_fixed, "live" to R.string.teams_dataset_live, "edit" to R.string.teams_dataset_edit)
                    modes.forEach { (value, label) ->
                        Row {
                            RadioButton(selected = mode == value, enabled = !busy && submission == null, onClick = { mode = value })
                            Text(stringResource(label))
                        }
                    }
                    selected.fields.forEach { field ->
                        Row {
                            Checkbox(checked = field in fields, enabled = !busy && submission == null,
                                onCheckedChange = { fields = if (it) fields + field else fields - field })
                            Text(field)
                        }
                    }
                }
                if (busy) CircularProgressIndicator()
            }
        },
        confirmButton = {
            choice?.let { selected ->
                TextButton(enabled = !busy && fields.isNotEmpty(), onClick = { scope.launch { publish(selected) } }) {
                    Text(stringResource(if (submission == null) R.string.teams_dataset_share else R.string.teams_dataset_continue))
                }
            }
        },
        dismissButton = { TextButton(enabled = !busy, onClick = close) { Text(stringResource(R.string.common_close)) } },
    )
}
