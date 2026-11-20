package com.lingxi.code.teams

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject
import kotlin.math.abs

internal val reportLink = Regex("""lingxi-data:release/([a-zA-Z0-9_-]{1,64})/([a-f0-9]{64})""")

@Composable
internal fun TeamReportButton(link: MatchResult, conversation: String, model: TeamMessagingModel) {
    val (_, dataset, pinned) = link.groupValues
    var open by remember(dataset, pinned, conversation) { mutableStateOf(false) }
    TextButton(onClick = { open = true }) { Text(stringResource(R.string.teams_report_open)) }
    if (!open) return
    var latest by remember { mutableStateOf(false) }
    var report by remember { mutableStateOf<JSONObject?>(null) }
    var status by remember { mutableIntStateOf(0) }
    var busy by remember { mutableStateOf(false) }
    val coroutine = rememberCoroutineScope()
    LaunchedEffect(dataset, pinned, conversation, latest) {
        report = null
        status = 0
        try {
            val answer = model.data(conversation, JSONObject().put("action", "getReport").put("dataset", dataset)
                .put("release", if (latest) JSONObject.NULL else pinned))
            require(answer.getString("dataset") == dataset && (latest || answer.getString("release") == pinned))
            report = answer
        } catch (error: CancellationException) { throw error }
        catch (_: Exception) { status = R.string.teams_data_unavailable }
    }
    AlertDialog(onDismissRequest = { open = false }, title = {
        Text(report?.optJSONObject("spec")?.optString("title") ?: stringResource(R.string.teams_report_open))
    }, text = {
        Column(Modifier.verticalScroll(rememberScrollState())) {
            report?.let { data ->
                val points = data.getJSONArray("points")
                val maximum = (0 until points.length()).maxOfOrNull { abs(points.getJSONObject(it).getString("value").toDouble()) } ?: 1.0
                for (i in 0 until points.length()) {
                    val point = points.getJSONObject(i)
                    Text(point.getString("group") + " · " + point.getString("value"))
                    val fraction = if (maximum == 0.0) 0f else (abs(point.getString("value").toDouble()) / maximum).toFloat()
                    LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
                }
                if (data.optBoolean("canReview")) {
                    val cells = data.getJSONArray("cells")
                    for (i in 0 until cells.length()) {
                        val cell = cells.getJSONObject(i)
                        TextButton(enabled = !busy, onClick = {
                            busy = true
                            coroutine.launch {
                                try {
                                    model.data(conversation, JSONObject().put("action", "openReportReview").put("dataset", dataset)
                                        .put("release", data.getString("release")).put("partition", cell.getString("partition")).put("group", cell.getString("group")))
                                    status = R.string.teams_report_reviewed
                                } catch (error: CancellationException) { throw error }
                                catch (_: Exception) { report = null; status = R.string.teams_data_unavailable }
                                finally { busy = false }
                            }
                        }) { Text(stringResource(R.string.teams_report_review) + " · " + cell.getString("partition") + " / " + cell.getString("group")) }
                    }
                }
            }
            if (status != 0) Text(stringResource(status))
        }
    }, confirmButton = { TextButton(onClick = { open = false }) { Text(stringResource(android.R.string.ok)) } },
        dismissButton = { TextButton(enabled = !busy, onClick = { latest = !latest }) {
            Text(stringResource(if (latest) R.string.teams_report_open else R.string.teams_report_latest))
        } })
}
