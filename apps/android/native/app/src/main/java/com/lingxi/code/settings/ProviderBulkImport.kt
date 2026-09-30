package com.lingxi.code.settings

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

@Composable
internal fun ProviderBulkImport(bridge: SettingsEngineBridge, layer: String) {
    val state by bridge.state.collectAsState()
    val context = LocalContext.current
    val coroutine = rememberCoroutineScope()
    var expanded by remember(state.generation,layer) { mutableStateOf(false) }
    var text by remember(state.generation,layer) { mutableStateOf("") }
    var preview by remember(state.generation,layer) { mutableStateOf<ProviderImportResult?>(null) }
    var baseline by remember(state.generation,layer) { mutableStateOf(JSONObject()) }
    val selected = remember(state.generation,layer) { mutableStateMapOf<String,Boolean>() }
    var error by remember(state.generation,layer) { mutableStateOf<String?>(null) }
    var busy by remember(state.generation,layer) { mutableStateOf(false) }
    ReportSettingsDraft("provider-import/$layer",text.isNotBlank() || preview != null || busy)
    val locked = layer == "managed" || "providers" in state.snapshot?.locked.orEmpty()
    val filePicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) coroutine.launch {
            runCatching {
                withContext(Dispatchers.IO) {
                    context.contentResolver.openInputStream(uri)?.bufferedReader()?.use { reader ->
                        val buffer = CharArray(2_000_001)
                        var count = 0
                        while (count < buffer.size) { val n = reader.read(buffer,count,buffer.size-count); if (n < 0) break; count += n }
                        require(count <= 2_000_000) { "Provider import exceeds 2 MB" }
                        String(buffer,0,count)
                    } ?: throw IllegalStateException("Unable to read selected file")
                }
            }.onSuccess { text=it;preview=null;selected.clear();error=null }
                .onFailure { error="Could not read the JSON file. Select a UTF-8 JSON file under 2 MB." }
        }
    }
    Column(verticalArrangement=Arrangement.spacedBy(8.dp)) {
        TextButton(onClick={expanded=!expanded}) {Text(settingsLabel("Import providers"))}
        if ((text.isNotBlank() || preview != null) && !busy) {
            TextButton(onClick={text="";preview=null;selected.clear();error=null}) { Text(settingsLabel("Discard draft and reload")) }
        }
        if (expanded) {
            Text(settingsLabel("Paste LingXi providers JSON or OpenCode provider JSON. Review the selected profiles before importing."))
            if (preview == null) {
                OutlinedTextField(text,{text=it;error=null},label={Text(settingsLabel("Provider import JSON"))},readOnly=locked || busy,modifier=Modifier.fillMaxWidth())
                TextButton(enabled=!locked && !busy,onClick={filePicker.launch(arrayOf("application/json","text/plain","application/octet-stream"))}) {Text(settingsLabel("Choose JSON file"))}
                Button(enabled=!locked && !busy && text.isNotBlank(),onClick={
                    runCatching {
                        baseline=JSONObject(state.snapshot?.layersJson ?: "{}").optJSONObject(layer)?.optJSONObject("providers") ?: JSONObject()
                        parseProviderImport(text,baseline)
                    }.onSuccess { result ->
                        preview=result;selected.clear();result.entries.forEach {selected[it.id]=!it.conflict && it.error==null};if(result.entries.none {it.error!=null}) text="";error=null
                    }.onFailure {error=it.message ?: "Invalid import JSON"}
                }) {Text(settingsLabel("Preview import"))}
            }
            preview?.let { result ->
                result.warnings.forEach {Text(settingsLabel(it))}
                result.entries.forEach { entry ->
                    Row {
                        Checkbox(selected[entry.id]==true,{selected[entry.id]=it},enabled=!locked && !busy && entry.error==null)
                        Text(entry.id,modifier=Modifier.weight(1f))
                    }
                    if (entry.conflict) Text(settingsLabel("Replaces an existing profile when selected."))
                    entry.error?.let {Text(it,color=MaterialTheme.colorScheme.error)}
                    entry.warnings.forEach {Text(settingsLabel(it))}
                    if (entry.credential != null) Text(settingsLabel("API key detected; it will be stored separately in secure credential storage."), modifier=Modifier.testTag("provider-import-secure-credential"))
                    if (entry.error==null) SelectionContainer {Text(entry.definition.toString(2))}
                }
                Button(enabled=!locked && !busy && selected.values.any {it},onClick={coroutine.launch {
                    busy=true
                    runCatching {bridge.importProviders(layer,baseline,result.entries.filter {selected[it.id]==true})}
                        .onSuccess {preview=null;selected.clear();text="";error=null}
                        .onFailure {error=it.message ?: "Import failed. The review is retained so you can retry."}
                    busy=false
                }}) {Text(settingsLabel("Import selected providers"))}
                TextButton(enabled=!busy,onClick={
                    // Return only sanitized definitions to the editor; never reinsert extracted keys.
                    if (text.isBlank()) text=JSONObject().put("providers",JSONObject().also { map -> result.entries.filter {it.error==null}.forEach {map.put(it.id,it.definition)} }).toString(2)
                    preview=null;selected.clear();error=null
                }) {Text(settingsLabel(if (result.entries.any {it.error!=null}) "Edit import JSON" else "Edit sanitized JSON"))}
            }
            if (locked) Text(settingsLabel("Managed policy: read-only"))
            error?.let {Text(settingsLabel(it),color=MaterialTheme.colorScheme.error)}
        }
    }
}
