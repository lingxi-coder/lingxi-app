package com.lingxi.code.teams

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
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
import java.math.BigDecimal
import java.math.BigInteger
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

@Composable
internal fun TeamDatasetButton(reference: DatasetResourceRef, grant: String, conversation: String, model: TeamMessagingModel) {
    var open by remember(reference, grant, conversation) { mutableStateOf(false) }
    TextButton(onClick = { open = true }) {
        Text(stringResource(if (reference is DatasetResourceRef.Dataset) R.string.teams_dataset_open_live else R.string.teams_dataset_open_fixed))
    }
    if (open) TeamDatasetDialog(reference, grant, conversation, model) { open = false }
}

private data class DatasetRead(val dataset: String, val command: JSONObject)
private data class EditableDatasetRow(val reference: DatasetResourceRef.Record, val partition: String, val value: String)
private data class DatasetPage(val title: String, val text: String, val next: Any?, val editable: List<EditableDatasetRow> = emptyList())
private data class DatasetEdit(val row: EditableDatasetRow, val value: String = row.value, val operation: String = UUID.randomUUID().toString())

private fun readRequest(reference: DatasetResourceRef, next: Any?): DatasetRead = when (reference) {
    is DatasetResourceRef.Dataset -> DatasetRead(reference.dataset, JSONObject().put("action", "list").apply {
        if (next is String) put("after", next)
    })
    is DatasetResourceRef.Record -> DatasetRead(reference.dataset, JSONObject().put("action", "read").put("reference",
        JSONObject().put("kind", "record").put("dataset", reference.dataset).put("record", reference.record)
            .put("revision", reference.revision).put("schemaVersion", reference.schemaVersion)))
    is DatasetResourceRef.Snapshot -> DatasetRead(reference.dataset, JSONObject().put("action", "openSnapshot")
        .put("snapshot", reference.snapshot).put("block", next as? Int ?: 0))
    is DatasetResourceRef.ReportRelease -> DatasetRead(reference.dataset, JSONObject().put("action", "openReport")
        .put("release", reference.release).apply { if (next is String) put("after", next) })
    is DatasetResourceRef.PageVersion -> throw TeamMessageFailure("INVALID_DATA")
}

private fun rows(values: JSONArray, display: (JSONObject) -> String): String =
    (0 until values.length()).joinToString("\n\n") { display(values.getJSONObject(it)) }

private fun readPage(reference: DatasetResourceRef, dataset: String, result: JSONObject, editable: Boolean): DatasetPage {
    if (reference is DatasetResourceRef.ReportRelease) {
        val metric = result.getJSONObject("metric")
        val scale = if (metric.getString("kind") == "sum") metric.getInt("scale") else 0
        if (scale !in 0..12) throw TeamMessageFailure("INVALID_DATA")
        val text = rows(result.getJSONArray("groups")) {
            it.getString("group") + "  " + BigDecimal(BigInteger(it.getString("units")), scale).toPlainString()
        }
        return DatasetPage(result.getString("title"), text, result.opt("cursor") as? String)
    }
    if (reference is DatasetResourceRef.Record) {
        if (DatasetResourceRef.parse(result.getJSONObject("reference")) != reference) throw TeamMessageFailure("INVALID_DATA")
        return DatasetPage(reference.record, result.getJSONObject("value").toString(2), null)
    }
    val records = result.getJSONArray("records")
    val changes = mutableListOf<EditableDatasetRow>()
    val text = rows(records) {
        val key = it.getJSONObject("reference").getString("record")
        val value = it.getJSONObject("value").toString(2)
        if (editable && !it.optBoolean("archived")) {
            val record = DatasetResourceRef.parse(it.getJSONObject("reference")) as? DatasetResourceRef.Record
                ?: throw TeamMessageFailure("INVALID_DATA")
            if (record.dataset != dataset) throw TeamMessageFailure("INVALID_DATA")
            changes.add(EditableDatasetRow(record, it.getString("partition"), value))
        }
        key + "\n" + value
    }
    val next = if (reference is DatasetResourceRef.Snapshot) (result.opt("nextBlock") as? Number)?.toInt()
    else result.opt("cursor") as? String
    return DatasetPage(dataset, text, next, changes)
}

@Composable
private fun TeamDatasetDialog(reference: DatasetResourceRef, grant: String, conversation: String, model: TeamMessagingModel, close: () -> Unit) {
    var page by remember { mutableStateOf<DatasetPage?>(null) }
    var next by remember { mutableStateOf<Any?>(null) }
    var attempt by remember { mutableStateOf(0) }
    var problem by remember { mutableStateOf<Int?>(null) }
    var saving by remember { mutableStateOf(false) }
    var edit by remember { mutableStateOf<DatasetEdit?>(null) }
    val coroutine = rememberCoroutineScope()

    fun request(dataset: String, command: JSONObject, operation: String? = null): JSONObject =
        JSONObject().put("protocol", DatasetResourceRef.PROTOCOL_VERSION)
            .put("dataset", dataset).put("via", grant).put("command", command).apply {
                if (operation != null) put("operation", operation)
            }

    LaunchedEffect(reference, grant, conversation, attempt) {
        page = null
        problem = null
        edit = null
        try {
            val read = readRequest(reference, next)
            val editable = reference is DatasetResourceRef.Dataset &&
                model.dataset(conversation, request(read.dataset, JSONObject().put("action", "describe"))).optString("access") == "edit"
            page = readPage(reference, read.dataset, model.dataset(conversation, request(read.dataset, read.command)), editable)
        } catch (cancel: CancellationException) {
            throw cancel
        } catch (_: Exception) {
            problem = R.string.teams_data_unavailable
        }
    }

    fun save(change: DatasetEdit) {
        if (saving) return
        val value = runCatching { JSONObject(change.value) }.getOrElse {
            problem = R.string.teams_data_unavailable
            return
        }
        val row = change.row.reference
        val put = JSONObject().put("action", "put").put("record", row.record).put("partition", change.row.partition)
            .put("expectedRevision", row.revision).put("schemaVersion", row.schemaVersion).put("value", value)
        saving = true
        problem = null
        coroutine.launch {
            try {
                model.dataset(conversation, request(row.dataset, put, change.operation))
                attempt++
            } catch (cancel: CancellationException) {
                throw cancel
            } catch (failure: Exception) {
                val code = (failure as? TeamMessageFailure)?.code
                problem = if (code == "DATA_CONFLICT") R.string.teams_data_conflict else R.string.teams_data_unavailable
                if (code in setOf("INVALID_SESSION", "CONVERSATION_REQUIRED", "DATA_ACCESS_REQUIRED")) {
                    page = null
                    edit = null
                }
            } finally { saving = false }
        }
    }

    AlertDialog(
        onDismissRequest = { if (!saving) close() },
        title = { Text(page?.title ?: stringResource(R.string.teams_dataset_title)) },
        text = {
            val current = page
            Column(Modifier.verticalScroll(rememberScrollState())) {
                problem?.let { Text(stringResource(it)) }
                val change = edit
                if (change != null) {
                    OutlinedTextField(value = change.value, enabled = !saving,
                        onValueChange = { edit = change.copy(value = it, operation = UUID.randomUUID().toString()) },
                        label = { Text(change.row.reference.record) })
                } else if (current != null) {
                    SelectionContainer { Text(if (current.text.isEmpty()) stringResource(R.string.teams_dataset_no_rows) else current.text) }
                    current.editable.forEach { row ->
                        TextButton(onClick = { edit = DatasetEdit(row); problem = null }) {
                            Text(stringResource(R.string.teams_dataset_edit_record) + " " + row.reference.record)
                        }
                    }
                } else if (problem == null) {
                    CircularProgressIndicator()
                }
            }
        },
        dismissButton = {
            val following = page?.next
            if (edit != null) {
                TextButton(enabled = !saving, onClick = { if (problem == R.string.teams_data_conflict) attempt++ else { edit = null; problem = null } }) {
                    Text(stringResource(if (problem == R.string.teams_data_conflict) R.string.common_retry else R.string.common_cancel))
                }
            } else if (following != null) TextButton(onClick = { next = following; attempt++ }) { Text(stringResource(R.string.teams_dataset_next)) }
            else if (problem != null || (page != null && reference is DatasetResourceRef.Dataset)) {
                TextButton(onClick = { attempt++ }) { Text(stringResource(R.string.common_retry)) }
            }
        },
        confirmButton = {
            val change = edit
            if (change == null) TextButton(onClick = close) { Text(stringResource(R.string.common_close)) }
            else TextButton(enabled = !saving && problem != R.string.teams_data_conflict, onClick = { save(change) }) {
                Text(stringResource(R.string.teams_dataset_save_record))
            }
        },
    )
}
