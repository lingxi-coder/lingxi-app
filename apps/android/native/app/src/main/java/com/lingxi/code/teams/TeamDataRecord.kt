package com.lingxi.code.teams

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

internal data class TeamDataRef(val dataset: String, val record: String, val revision: Long, val schemaVersion: Long) {
    fun json(): JSONObject = JSONObject().put("dataset", dataset).put("record", record)
        .put("revision", revision).put("schemaVersion", schemaVersion)

    companion object {
        private val id = Regex("[A-Za-z0-9_-]{1,64}")
        fun parse(value: JSONObject?): TeamDataRef? {
            if (value == null || value.length() != 4) return null
            val dataset = value.opt("dataset") as? String ?: return null
            val record = value.opt("record") as? String ?: return null
            fun version(key: String): Long? {
                val number = value.opt(key) as? Number ?: return null
                val integer = number.toLong()
                return integer.takeIf { it in 1..9007199254740991L && number.toDouble() == it.toDouble() }
            }
            if (!id.matches(dataset) || !id.matches(record)) return null
            return TeamDataRef(dataset, record, version("revision") ?: return null, version("schemaVersion") ?: return null)
        }
    }
}

@Composable
internal fun TeamDataRecordButton(reference: TeamDataRef, conversationId: String, model: TeamMessagingModel) {
    var open by remember(reference, conversationId) { mutableStateOf(false) }
    TextButton(onClick = { open = true }) { Text(stringResource(R.string.teams_data_open)) }
    if (open) TeamDataRecordDialog(reference, conversationId, model, onClose = { open = false })
}

@Composable
private fun TeamDataRecordDialog(reference: TeamDataRef, conversationId: String, model: TeamMessagingModel, onClose: () -> Unit) {
    var revision by remember(reference) { mutableStateOf(reference) }
    var value by remember(reference) { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf(false) }
    var canEdit by remember { mutableStateOf(false) }
    var views by remember(reference) { mutableStateOf<List<JSONObject>>(emptyList()) }
    var openView by remember(reference) { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<Int?>(null) }
    var saving by remember { mutableStateOf(false) }
    var pending by remember { mutableStateOf<Pair<String, String>?>(null) }
    val coroutine = rememberCoroutineScope()
    fun showFailure(failure: Exception) {
        val code = (failure as? TeamMessageFailure)?.code
        if (code in setOf("DATA_ACCESS_REQUIRED", "INVALID_SESSION", "CONVERSATION_REQUIRED", "MEMBERSHIP_REQUIRED", "STALE_MEMBERSHIP")) {
            value = null; canEdit = false; editing = false; pending = null; views = emptyList()
        }
        error = if (code == "DATA_CONFLICT") R.string.teams_data_conflict else R.string.teams_data_unavailable
    }
    LaunchedEffect(revision) {
        value = null
        views = emptyList()
        openView = null
        canEdit = false
        try {
            val reply = model.data(conversationId, JSONObject().put("action", "get").put("reference", revision.json()))
            if (TeamDataRef.parse(reply.optJSONObject("reference")) != revision) throw TeamMessageFailure("INVALID_DATA")
            val record = reply.getJSONObject("record")
            val allowed = model.data(conversationId, JSONObject().put("action", "datasets")).getJSONArray("datasets")
            try {
                val templates = model.data(conversationId, JSONObject().put("action", "templateViews").put("dataset", revision.dataset))
                views = templates.getJSONArray("views").objects().filter { it.optLong("schemaVersion") == revision.schemaVersion }
            } catch (failure: TeamMessageFailure) {
                if (failure.code != "INVALID_DATA_REQUEST") throw failure
            }
            canEdit = !record.optBoolean("archived") && allowed.objects().any {
                it.optString("dataset") == revision.dataset && it.optString("access") in setOf("edit", "manage")
            }
            value = record.getJSONObject("value").toString(2)
            error = null
        } catch (cancel: CancellationException) { throw cancel }
        catch (_: Exception) { value = null; canEdit = false; error = R.string.teams_data_unavailable }
    }
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(revision.record) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                error?.let { Text(stringResource(it)) }
                if (error == R.string.teams_data_conflict) TextButton(enabled = !saving, onClick = {
                    coroutine.launch {
                        saving = true
                        try {
                            val latest = model.data(conversationId, JSONObject().put("action", "latest")
                                .put("dataset", revision.dataset).put("record", revision.record))
                            val reference = TeamDataRef.parse(latest) ?: throw TeamMessageFailure("INVALID_DATA")
                            if (reference.dataset != revision.dataset || reference.record != revision.record) throw TeamMessageFailure("INVALID_DATA")
                            if (reference != revision) {
                                revision = reference
                                error = null; editing = false; pending = null
                            }
                        } catch (cancel: CancellationException) { throw cancel }
                        catch (failure: Exception) { showFailure(failure) }
                        finally { saving = false }
                    }
                }) { Text(stringResource(R.string.teams_data_latest)) }
                val content = value
                if (content != null && editing) {
                    OutlinedTextField(value = content, onValueChange = { value = it; error = null }, enabled = !saving,
                        label = { Text(stringResource(R.string.teams_data_json)) })
                } else if (content != null) {
                    if (views.isEmpty()) Text(content)
                    else {
                        TextButton(onClick = { openView = null }) { Text(stringResource(R.string.teams_data_json)) }
                        views.forEach { view ->
                            val name = view.optString("view")
                            TextButton(onClick = { openView = name }) { Text(name) }
                        }
                        val spec = views.firstOrNull { it.optString("view") == openView }?.optJSONObject("definition")
                        if (spec?.optInt("version") == 1) {
                            val fields = JSONObject(content)
                            spec.optJSONArray("nodes")?.objects()?.forEach { TeamDataTemplateNode(it, fields, 0) }
                        } else Text(content)
                    }
                }
                else if (error == null) Text(stringResource(R.string.teams_data_loading))
            }
        },
        confirmButton = {
            if (canEdit && value != null) TextButton(enabled = !saving, onClick = {
                if (!editing) editing = true
                else coroutine.launch {
                    val draft = value ?: return@launch
                    val row = try { JSONObject(draft) } catch (_: Exception) { error = R.string.teams_data_invalid; return@launch }
                    val operation = pending?.takeIf { it.first == draft }?.second ?: UUID.randomUUID().toString()
                    pending = draft to operation
                    saving = true; error = null
                    try {
                        val result = model.data(conversationId, JSONObject().put("action", "put")
                            .put("dataset", revision.dataset).put("record", revision.record).put("schemaVersion", revision.schemaVersion)
                            .put("expectedRevision", revision.revision).put("value", row), operation)
                        revision = TeamDataRef.parse(result) ?: throw TeamMessageFailure("INVALID_DATA")
                        editing = false; pending = null
                    } catch (cancel: CancellationException) { throw cancel }
                    catch (failure: Exception) { showFailure(failure) }
                    finally { saving = false }
                }
            }) { Text(stringResource(if (editing) R.string.teams_data_save else R.string.teams_message_edit)) }
        },
        dismissButton = { TextButton(onClick = onClose) { Text(stringResource(android.R.string.cancel)) } },
    )
}

@Composable
private fun TeamDataTemplateNode(node: JSONObject, fields: JSONObject, depth: Int, emphasis: String = "") {
    if (depth > 10) return
    val text = when (node.optString("kind")) {
        "text" -> node.optString("text")
        "field" -> fields.opt(node.optString("field"))?.toString() ?: "—"
        else -> null
    }
    if (text != null) {
        Text(text, style = if (emphasis == "h3") MaterialTheme.typography.titleMedium else MaterialTheme.typography.bodyMedium,
            fontWeight = if (emphasis == "strong") FontWeight.Bold else null)
        return
    }
    if (node.optString("kind") != "element") return
    val tag = node.optString("tag")
    if (tag !in setOf("section", "div", "h3", "p", "span", "strong", "small", "ul", "li")) return
    val children = node.optJSONArray("children")?.objects() ?: return
    val inherited = if (tag == "h3" || tag == "strong") tag else emphasis
    val style = node.optJSONObject("style")
    if (style?.optString("display") == "flex" && style.optString("flexDirection") != "column") {
        Row { children.forEach { TeamDataTemplateNode(it, fields, depth + 1, inherited) } }
    } else {
        Column { children.forEach { TeamDataTemplateNode(it, fields, depth + 1, inherited) } }
    }
}
