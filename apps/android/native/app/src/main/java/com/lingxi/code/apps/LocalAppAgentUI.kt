package com.lingxi.code.apps

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.json.JSONObject

/** This dialog belongs to native app details, never to the application page. */
@Composable
internal fun LocalAppAgentSettingsDialog(apps: UnifiedLocalApps, release: AppRelease, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var catalog by remember(release.digest) { mutableStateOf<JSONObject?>(null) }
    var selected by remember(release.digest) { mutableStateOf("") }
    var granted by remember(release.digest) { mutableStateOf(setOf<String>()) }
    var turns by remember(release.digest) { mutableIntStateOf(20) }
    var minutes by remember(release.digest) { mutableIntStateOf(10) }
    var error by remember(release.digest) { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val declared = release.manifest.optJSONObject("agent")?.optJSONArray("tools").strings()
    fun key(model: JSONObject) = model.getString("providerId") + "/" + model.getString("modelId")
    LaunchedEffect(release.digest) {
        try {
            catalog = apps.agentHost(JSONObject().put("op", "catalog")) as JSONObject
            val saved = apps.agentHost(JSONObject().put("op", "settings").put("appId", release.appId)) as? JSONObject
            if (saved != null) {
                selected = key(saved.getJSONObject("model"))
                granted = saved.getJSONObject("tools").keys().asSequence().toSet()
                turns = saved.getInt("maxTurns"); minutes = (saved.getLong("timeoutMs") / 60_000).toInt().coerceAtLeast(1)
            }
        } catch (failure: Exception) { error = failure.message }
    }
    fun save(revoke: Boolean) { scope.launch {
        busy = true; error = null
        try {
            val current = catalog ?: throw IllegalStateException("宿主模型目录尚未就绪")
            val models = current.getJSONArray("models")
            val model = (0 until models.length()).map { models.getJSONObject(it) }.firstOrNull { key(it) == selected && it.optBoolean("canSelect") }
            val tools = JSONObject(); granted.filter { it in declared && current.getJSONObject("tools").has(it) }.forEach { tools.put(it, current.getJSONObject("tools").getJSONObject(it)) }
            // Catalog labels/capabilities are presentation metadata. The
            // strict native settings contract accepts only the model identity.
            val modelRef = model?.let { JSONObject().put("providerId", it.getString("providerId")).put("modelId", it.getString("modelId")) }
            val settings = if (revoke) JSONObject.NULL else JSONObject().put("model", modelRef ?: throw IllegalStateException("请选择可用模型")).put("tools", tools)
                .put("background", false).put("maxTurns", turns).put("timeoutMs", minutes * 60_000)
            apps.agentHost(JSONObject().put("op", "configure").put("appId", release.appId).put("settings", settings))
            onDismiss()
        } catch (failure: Exception) { error = failure.message }
        finally { busy = false }
    } }
    AlertDialog(onDismissRequest = { if (!busy) onDismiss() }, title = { Text("Agent 授权") }, text = {
        Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("独立任务使用你在灵犀配置的模型账号，不继承创建应用的聊天。")
            val current = catalog
            if (current == null && error == null) CircularProgressIndicator()
            if (current != null) {
                Text("模型", style = MaterialTheme.typography.titleSmall)
                val models = current.getJSONArray("models")
                if ((0 until models.length()).none { models.getJSONObject(it).optBoolean("canSelect") }) {
                    Text("尚无可用模型。请先在灵犀设置中配置模型和账号。", style = MaterialTheme.typography.bodySmall)
                }
                (0 until models.length()).map { models.getJSONObject(it) }.filter { it.optBoolean("canSelect") }.forEach { model ->
                    Row(Modifier.fillMaxWidth().clickable { selected = key(model) }) {
                        RadioButton(selected == key(model), onClick = { selected = key(model) })
                        Text(model.getString("label") + " · " + model.getString("providerId"), Modifier.padding(top = 12.dp))
                    }
                }
                val servers = current.optJSONArray("servers")
                if (servers != null && servers.length() > 0) {
                    Text("已有 MCP 连接", style = MaterialTheme.typography.titleSmall)
                    (0 until servers.length()).map { servers.getJSONObject(it) }.forEach { server ->
                        if (server.optBoolean("available")) TextButton(enabled = !busy, onClick = { scope.launch {
                            busy = true; error = null
                            try { catalog = apps.agentHost(JSONObject().put("op", "connectMcp").put("serverId", server.getString("id"))) as JSONObject }
                            catch (failure: Exception) { error = failure.message }
                            finally { busy = false }
                        } }) { Text("读取 " + server.getString("name") + " 的工具目录") }
                        else Text(server.getString("name") + "：此连接需要桌面端")
                    }
                    Text("连接后仍需逐项授权本应用请求的工具。", style = MaterialTheme.typography.bodySmall)
                }
                Text("允许此应用使用的工具", style = MaterialTheme.typography.titleSmall)
                if (declared.isEmpty()) Text("此版本不请求业务工具。")
                declared.filter { current.getJSONObject("tools").has(it) }.forEach { id ->
                    Row(Modifier.fillMaxWidth()) {
                        Checkbox(id in granted, onCheckedChange = { allowed -> granted = if (allowed) granted + id else granted - id })
                        Text(current.optJSONObject("labels")?.optString(id, id) ?: id, Modifier.padding(top = 12.dp))
                    }
                }
                Text("敏感操作仍由灵犀请求批准；工具或连接配置变化后需要重新授权。", style = MaterialTheme.typography.bodySmall)
                Row { Text("最多 $turns 轮", Modifier.weight(1f)); TextButton(onClick = { turns = (turns - 1).coerceAtLeast(1) }) { Text("−") }; TextButton(onClick = { turns = (turns + 1).coerceAtMost(100) }) { Text("＋") } }
                Row { Text("最多 $minutes 分钟", Modifier.weight(1f)); TextButton(onClick = { minutes = (minutes - 1).coerceAtLeast(1) }) { Text("−") }; TextButton(onClick = { minutes = (minutes + 1).coerceAtMost(60) }) { Text("＋") } }
                Text("停止应用或撤销授权会停止任务。浏览器页面关闭不等于停止；模型费用按实际用量计量。", style = MaterialTheme.typography.bodySmall)
                TextButton(enabled = !busy, onClick = { save(true) }) { Text("撤销 Agent 授权") }
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }, confirmButton = { TextButton(enabled = catalog != null && selected.isNotEmpty() && !busy, onClick = { save(false) }) { Text("保存授权") } },
        dismissButton = { TextButton(enabled = !busy, onClick = onDismiss) { Text("取消") } })
}

/** Trusted UI proposal polling; application JavaScript cannot obtain/answer it. */
@Composable
internal fun LocalAppAgentInteractions(apps: UnifiedLocalApps, appId: String) {
    val scope = rememberCoroutineScope()
    var pending by remember(appId) { mutableStateOf<JSONObject?>(null) }
    var error by remember(appId) { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(appId) {
        while (true) {
            try {
                val requests = apps.agentHost(JSONObject().put("op", "interactions")) as org.json.JSONArray
                val current = (0 until requests.length()).map { requests.getJSONObject(it) }.firstOrNull { it.optString("appId") == appId }
                if (current?.optString("requestId") != pending?.optString("requestId")) pending = current
            } catch (cancelled: kotlinx.coroutines.CancellationException) { throw cancelled }
            catch (_: Exception) { /* A get/run call exposes unavailable host errors in the app. */ }
            kotlinx.coroutines.delay(750)
        }
    }
    val request = pending ?: return
    var selected by remember(request.getString("requestId")) { mutableStateOf(mapOf<String, Set<String>>()) }
    var custom by remember(request.getString("requestId")) { mutableStateOf(mapOf<String, String>()) }
    val permission = request.getString("type") == "permission"
    val questions = request.optJSONArray("questions")?.let { list -> (0 until list.length()).map { list.getJSONObject(it) } }.orEmpty()
    val complete = questions.all { custom[it.getString("question")].orEmpty().isNotBlank() || selected[it.getString("question")].orEmpty().isNotEmpty() }
    fun respond(approved: Boolean) { scope.launch {
        busy = true; error = null
        try {
            val answer = JSONObject().put("op", if (!approved && !permission) "cancel" else "answer")
                .put("runId", request.getString("runId")).put("requestId", request.getString("requestId"))
            if (permission) answer.put("approved", approved)
            else if (approved) {
                val values = JSONObject()
                questions.forEach { question ->
                    val key = question.getString("question")
                    val options = question.getJSONArray("options")
                    val chosen = (0 until options.length()).map { options.getJSONObject(it).getString("label") }.filter { it in selected[key].orEmpty() }
                    values.put(key, custom[key].orEmpty().trim().ifEmpty { chosen.joinToString(", ") })
                }
                answer.put("answers", values)
            }
            apps.agentHost(answer); pending = null
        } catch (failure: Exception) { error = failure.message }
        finally { busy = false }
    } }
    AlertDialog(onDismissRequest = {}, title = { Text(if (permission) "灵犀工具审批" else "Agent 需要回答") }, text = {
        Column(Modifier.heightIn(max = 560.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("应用：${request.getString("appId")}", style = MaterialTheme.typography.bodySmall)
            if (permission) {
                Text(request.optString("toolName", "Agent"), style = MaterialTheme.typography.titleMedium)
                Text(request.optJSONObject("input")?.toString(2) ?: request.optString("input"), style = MaterialTheme.typography.bodySmall)
            } else questions.forEach { question ->
                val key = question.getString("question")
                Text(key, style = MaterialTheme.typography.titleSmall)
                val options = question.getJSONArray("options")
                (0 until options.length()).map { options.getJSONObject(it) }.forEach { option ->
                    val label = option.getString("label")
                    Row(Modifier.fillMaxWidth()) {
                        Checkbox(label in selected[key].orEmpty(), onCheckedChange = { value ->
                            val before = selected[key].orEmpty()
                            selected = selected + (key to if (question.optBoolean("multiSelect")) { if (value) before + label else before - label } else { if (value) setOf(label) else emptySet() })
                        })
                        Column(Modifier.padding(top = 10.dp)) { Text(label); Text(option.optString("description"), style = MaterialTheme.typography.bodySmall) }
                    }
                }
                OutlinedTextField(custom[key].orEmpty(), onValueChange = { custom = custom + (key to it) }, label = { Text("其他回答") })
            }
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        }
    }, confirmButton = { TextButton(enabled = !busy && (permission || complete), onClick = { respond(true) }) { Text(if (permission) "仅允许此次操作" else "提交回答") } },
        dismissButton = { TextButton(enabled = !busy, onClick = { respond(false) }) { Text(if (permission) "拒绝" else "取消任务") } })
}
