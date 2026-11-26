package com.lingxi.code.teams

import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

private data class Configuration(val limit: Int, val revision: Long, val canEdit: Boolean)

@Composable
internal fun TeamVisualizationSettings(conversation: String, request: suspend (JSONObject) -> JSONObject) {
    val scope = rememberCoroutineScope()
    val ceiling = TeamVisualizationTransfer.maximumBytes / (1024 * 1024)
    var settings by remember(conversation) { mutableStateOf<Configuration?>(null) }
    var limit by remember(conversation) { mutableStateOf("") }
    var saving by remember(conversation) { mutableStateOf(false) }
    var failed by remember(conversation) { mutableStateOf(false) }
    fun decode(value: JSONObject): Configuration = Configuration(value.getInt("limitMiB"), value.getLong("revision"), value.getBoolean("canEdit"))
    suspend fun reload() {
        try {
            val current = decode(request(objectOf("action" to "settings")))
            settings = current
            limit = current.limit.toString()
        } catch (error: CancellationException) { throw error }
        catch (_: Exception) { failed = true }
    }
    LaunchedEffect(conversation) { reload() }
    settings?.let { configuration ->
        if (configuration.canEdit) {
            val size = limit.toIntOrNull()?.takeIf { it in 1..ceiling }
            OutlinedTextField(limit, { limit = it.take(2).filter(Char::isDigit) },
                label = { Text(stringResource(R.string.teams_visualization_limit)) },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), enabled = !saving, singleLine = true)
            TextButton(enabled = !saving && size != null && size != configuration.limit, onClick = {
                if (size == null || saving) return@TextButton
                saving = true
                failed = false
                scope.launch {
                    try {
                        settings = decode(request(objectOf("action" to "setLimit", "limitMiB" to size, "expectedRevision" to configuration.revision)))
                    } catch (error: CancellationException) { throw error }
                    catch (_: Exception) { reload(); failed = true }
                    finally { saving = false }
                }
            }) { Text(stringResource(R.string.common_save)) }
        } else {
            Text(stringResource(R.string.teams_visualization_limit) + " · " + configuration.limit, style = MaterialTheme.typography.bodyMedium)
        }
    }
    if (failed) Text(stringResource(R.string.teams_data_unavailable), color = MaterialTheme.colorScheme.error)
}
