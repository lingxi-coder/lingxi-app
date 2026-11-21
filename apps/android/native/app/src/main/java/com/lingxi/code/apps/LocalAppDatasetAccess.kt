package com.lingxi.code.apps

import android.content.Context
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.auth.FirebaseUser
import com.lingxi.code.BuildConfig
import java.net.HttpURLConnection
import java.net.URI
import java.security.MessageDigest
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import org.json.JSONObject

internal data class LocalAppDatasetBinding(val dataset: String = "", val grant: String = "")

internal object LocalAppDatasetAccess {
    val origin: String?
        get() {
            val url = runCatching { URI(BuildConfig.LINGXI_DATASET_BROWSER_ORIGIN) }.getOrNull() ?: return null
            return url.takeIf {
                it.scheme == "https" && it.host != null && it.userInfo == null && it.query == null &&
                    it.fragment == null && it.path.orEmpty() in listOf("", "/")
            }?.toString()?.trimEnd('/')
        }
    private val auth get() = FirebaseAuth.getInstance()
    fun uid(): String? = auth.currentUser?.uid

    fun declarations(release: AppRelease): Map<String, String> {
        val bindings = release.manifest.optJSONObject("dataBindings") ?: return emptyMap()
        return bindings.keys().asSequence().associateWith(bindings::getString)
    }

    private fun digest(value: String): String =
        MessageDigest.getInstance("SHA-256").digest(value.toByteArray()).joinToString("") { "%02x".format(it) }

    private fun key(uid: String, app: String, release: String): String = digest("$uid:$app:$release")
    private fun preferences(context: Context) = context.getSharedPreferences("local-app-dataset-bindings", Context.MODE_PRIVATE)

    fun saved(context: Context, release: AppRelease): Map<String, LocalAppDatasetBinding> {
        val uid = uid() ?: return emptyMap()
        val text = preferences(context).getString(key(uid, release.appId, release.digest), null) ?: return emptyMap()
        return runCatching {
            val raw = JSONObject(text)
            raw.keys().asSequence().associateWith { alias ->
                val value = raw.getJSONObject(alias)
                LocalAppDatasetBinding(value.getString("dataset"), value.optString("grant"))
            }
        }.getOrDefault(emptyMap())
    }

    suspend fun savedIntent(release: AppRelease, owner: String): Map<String, LocalAppDatasetBinding> {
        val origin = origin ?: return emptyMap()
        val user = auth.currentUser?.takeIf { it.uid == owner } ?: return emptyMap()
        val result = command(origin, user, JSONObject().put("action", "getIntent")
            .put("application", release.appId).put("release", release.digest)).optJSONObject("bindings") ?: return emptyMap()
        return declarations(release).mapNotNull { (alias, mode) ->
            val binding = result.optJSONObject(alias) ?: return@mapNotNull null
            val reference = binding.optJSONObject("reference") ?: return@mapNotNull null
            val dataset = reference.optString("dataset")
            if (reference.optString("kind") != "dataset" || dataset.isEmpty() || binding.optBoolean("edit") != (mode == "edit")) {
                return@mapNotNull null
            }
            alias to LocalAppDatasetBinding(dataset, if (binding.isNull("via")) "" else binding.optString("via"))
        }.toMap()
    }

    suspend fun save(context: Context, release: AppRelease, owner: String, bindings: Map<String, LocalAppDatasetBinding>, sync: Boolean) {
        if (sync) {
            val origin = origin ?: error("Dataset connection is unavailable.")
            val user = auth.currentUser?.takeIf { it.uid == owner } ?: error("The signed-in account changed.")
            command(origin, user, JSONObject().put("action", "saveIntent").put("application", release.appId)
                .put("release", release.digest).put("bindings", encode(release, bindings)))
        }
        check(uid() == owner) { "The signed-in account changed." }
        val encoded = JSONObject()
        for ((alias, binding) in bindings) {
            encoded.put(alias, JSONObject().put("dataset", binding.dataset).put("grant", binding.grant))
        }
        preferences(context).edit().putString(key(owner, release.appId, release.digest), encoded.toString()).apply()
    }

    suspend fun proof(context: Context, release: AppRelease, origin: String): String {
        val user = auth.currentUser ?: error("Sign in to open the Dataset connection.")
        val preferences = preferences(context)
        val storage = key(user.uid, release.appId, "installation")
        val installation = preferences.getString(storage, null) ?: UUID.randomUUID().toString().also {
            preferences.edit().putString(storage, it).apply()
        }
        val install = JSONObject().put("application", release.appId).put("installation", installation)
            .put("version", release.revision).put("release", release.digest).put("publisher", digest(installation))
        return command(origin, user, JSONObject().put("action", "launch").put("bindings", encode(release, saved(context, release)))
            .put("install", install)).getString("proof")
    }

    private fun encode(release: AppRelease, approved: Map<String, LocalAppDatasetBinding>): JSONObject {
        val bindings = JSONObject()
        for ((alias, mode) in declarations(release)) {
            val binding = approved[alias]?.takeIf { it.dataset.isNotBlank() }
                ?: error("Choose a Dataset for each declared app binding.")
            val value = JSONObject().put("reference", JSONObject().put("kind", "dataset").put("dataset", binding.dataset))
                .put("edit", mode == "edit")
            if (binding.grant.isNotEmpty()) value.put("via", binding.grant)
            bindings.put(alias, value)
        }
        return bindings
    }

    suspend fun revoke(context: Context, appId: String) {
        val origin = origin ?: return
        val user = auth.currentUser ?: return
        val installation = preferences(context).getString(key(user.uid, appId, "installation"), null) ?: return
        command(origin, user, JSONObject().put("action", "revoke").put("installation", installation))
    }

    private suspend fun command(origin: String, user: FirebaseUser, body: JSONObject): JSONObject {
        val token = user.getIdToken(false).await().token ?: error("Dataset sign-in is unavailable.")
        check(uid() == user.uid) { "The signed-in account changed." }
        val result = withContext(Dispatchers.IO) {
            val request = URI("$origin/v1/dataset-browser/host").toURL().openConnection() as HttpURLConnection
            try {
                request.requestMethod = "POST"
                request.instanceFollowRedirects = false
                request.connectTimeout = 15_000
                request.readTimeout = 15_000
                request.setRequestProperty("Content-Type", "application/json")
                request.setRequestProperty("Authorization", "Bearer $token")
                request.doOutput = true
                request.outputStream.use { it.write(body.toString().toByteArray()) }
                check(request.responseCode == 200) { "Dataset access is unavailable or has been revoked." }
                JSONObject(request.inputStream.bufferedReader().use { it.readText() })
            } finally { request.disconnect() }
        }
        check(uid() == user.uid) { "The signed-in account changed." }
        return result
    }
}

@Composable
internal fun LocalAppDatasetConnectionDialog(
    release: AppRelease,
    initial: Map<String, LocalAppDatasetBinding>,
    saving: Boolean,
    onSave: (String, Map<String, LocalAppDatasetBinding>, Boolean) -> Unit,
    onClose: () -> Unit,
) {
    val declarations = remember(release.digest) { LocalAppDatasetAccess.declarations(release) }
    val owner = remember(release.digest) { LocalAppDatasetAccess.uid() }
    var values by remember(release.digest) { mutableStateOf(initial) }
    var sync by remember(release.digest) { mutableStateOf(false) }
    LaunchedEffect(release.digest, owner) {
        if (owner != null && values.isEmpty()) {
            val draft = try { LocalAppDatasetAccess.savedIntent(release, owner) }
                catch (failure: kotlinx.coroutines.CancellationException) { throw failure }
                catch (_: Exception) { emptyMap() }
            if (values.isEmpty()) values = draft
        }
    }
    AlertDialog(
        onDismissRequest = { if (!saving) onClose() },
        title = { Text("连接数据集") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                for ((alias, mode) in declarations) {
                    Text(if (mode == "edit") "$alias · 读写" else "$alias · 读取")
                    val binding = values[alias] ?: LocalAppDatasetBinding()
                    OutlinedTextField(value = binding.dataset, enabled = !saving,
                        onValueChange = { values = values + (alias to binding.copy(dataset = it.trim())) },
                        label = { Text("数据集 ID") }, singleLine = true)
                    OutlinedTextField(value = binding.grant, enabled = !saving,
                        onValueChange = { values = values + (alias to binding.copy(grant = it.trim())) },
                        label = { Text("共享授权（可选）") }, singleLine = true)
                }
                Row(Modifier.toggleable(sync, enabled = !saving) { sync = it }) {
                    Checkbox(checked = sync, onCheckedChange = null, enabled = !saving)
                    Text("同步绑定意向到我的设备")
                }
            }
        },
        confirmButton = {
            TextButton(enabled = !saving && owner != null && declarations.keys.all { !values[it]?.dataset.isNullOrBlank() },
                onClick = { if (owner != null) onSave(owner, values, sync) }) { Text("授权并打开") }
        },
        dismissButton = { TextButton(enabled = !saving, onClick = onClose) { Text("取消") } },
    )
}
