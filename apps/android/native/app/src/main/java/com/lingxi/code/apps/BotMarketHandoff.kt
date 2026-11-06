package com.lingxi.code.apps

import android.content.Context
import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.firebase.auth.FirebaseAuth
import com.lingxi.code.BuildConfig
import com.lingxi.code.market.MarketClient
import com.lingxi.code.navigation.DeepLink
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import org.json.JSONObject
import java.net.URI
import java.util.UUID

internal fun requireBotMarketApiRoot(value: String): String = URI(value).also { root ->
    require(root.scheme == "https" && root.host != null && root.rawUserInfo == null && root.rawQuery == null && root.rawFragment == null && root.rawPath == "/api/v3/") { "Invalid Bot market endpoint" }
}.toString()

/** Only public identities and an authenticated receipt. No credentials or resource bindings. */
data class BotMarketHandoff(val listingId: String, val versionId: String? = null, val intentId: String? = null,
    val requestId: String = UUID.randomUUID().toString(), val status: String = "awaiting_desktop", val instanceId: String? = null) {
    init { require(validId(listingId)); require(versionId == null || validId(versionId)); require(intentId == null || validId(intentId)) }
    val desktopUrl: String get() = DeepLink.build("market/bot/$listingId", buildMap { versionId?.let { put("versionId", it) }; intentId?.let { put("intentId", it) } })
    val marketUrl: String get() = URI(BuildConfig.LOCAL_APP_MARKET_API_ROOT).let { URI(it.scheme, null, it.host, it.port, "/bots/$listingId", "platform=macos", null).toString() }
    fun save(context: Context) { preferences(context).edit().putString(key(), JSONObject().put("listingId", listingId).put("versionId", versionId).put("intentId", intentId).put("requestId", requestId).put("status", status).put("instanceId", instanceId).toString()).apply() }
    suspend fun resolve(context: Context, create: Boolean): BotMarketHandoff {
        val api = MarketClient(requireBotMarketApiRoot(BuildConfig.BOT_MARKET_API_ROOT))
        val user = FirebaseAuth.getInstance().currentUser ?: error("Sign in to track this Desktop download")
        val owner = user.uid
        val token = user.getIdToken(false).await().token ?: error("Sign in again")
        check(FirebaseAuth.getInstance().currentUser?.uid == owner) { "Account changed during handoff" }
        var version = versionId ?: pending(context)?.takeIf { it.requestId == requestId }?.versionId
        if (intentId == null && !create) return this
        if (intentId == null && version == null) version = api.authenticatedRequest("/bots/$listingId", token).getJSONObject("data").getJSONObject("currentVersion").getString("id")
        val result = if (intentId != null) api.authenticatedRequest("/bot-install-intents/$intentId", token).getJSONObject("data") else {
            check(FirebaseAuth.getInstance().currentUser?.uid == owner) { "Account changed during handoff" }
            copy(versionId = version).save(context) // fixed command identity is persisted before POST; ACK loss replays it.
            api.authenticatedRequest("/bot-install-intents", token, JSONObject().put("listingId", listingId).put("versionId", version), requestId).getJSONObject("data")
        }
        check(FirebaseAuth.getInstance().currentUser?.uid == owner) { "Account changed during handoff" }
        check(result.getString("listingId") == listingId && (version == null || result.getString("versionId") == version)) { "Handoff release changed" }
        check(intentId == null || result.getString("id") == intentId) { "Handoff identity changed" }
        val receiptStatus = result.getString("status")
        check(receiptStatus in setOf("awaiting_desktop", "installed")) { "Invalid Desktop receipt" }
        if (receiptStatus == "installed") check(result.optString("instanceId").matches(Regex("^[A-Za-z0-9_-]{22}$"))) { "Desktop receipt has no committed instance" }
        return copy(versionId = result.getString("versionId"), intentId = result.getString("id"), status = result.getString("status"), instanceId = result.optString("instanceId").takeIf { it.isNotBlank() && it != "null" }).also { it.save(context) }
    }
    companion object {
        private fun validId(value: String) = Regex("^[A-Za-z0-9_-]{1,128}$").matches(value)
        private fun preferences(context: Context) = context.getSharedPreferences("bot-market-handoffs", Context.MODE_PRIVATE)
        private fun key() = "pending:" + (FirebaseAuth.getInstance().currentUser?.uid ?: "anonymous")
        fun pending(context: Context): BotMarketHandoff? = runCatching {
            val json = JSONObject(preferences(context).getString(key(), null) ?: return null)
            fun optional(key: String) = json.optString(key).takeIf { it.isNotBlank() && it != "null" }
            BotMarketHandoff(json.getString("listingId"), optional("versionId"), optional("intentId"), json.getString("requestId"), json.getString("status"), optional("instanceId"))
        }.getOrNull()
    }
}

@Composable
fun BotMarketHandoffPage(listingId: String, versionId: String?, intentId: String?, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val owner = LocalLifecycleOwner.current
    var handoff by remember(listingId, versionId, intentId) { mutableStateOf(BotMarketHandoff.pending(context)?.takeIf { it.listingId == listingId && (versionId == null || it.versionId == versionId) && (intentId == null || it.intentId == intentId) } ?: BotMarketHandoff(listingId, versionId, intentId)) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    suspend fun refresh() { try { handoff = handoff.resolve(context, false); error = null } catch (failure: Exception) { error = failure.message } }
    LaunchedEffect(listingId, versionId, intentId) { handoff.save(context); if (handoff.intentId != null) refresh() }
    DisposableEffect(owner, handoff.intentId) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME && handoff.intentId != null) scope.launch { refresh() } }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        TextButton(onClick = onBack) { Text("Back") }
        Text(if (handoff.status == "installed") "Downloaded on Desktop" else "Continue on Desktop", style = MaterialTheme.typography.headlineSmall)
        Text("Choose your own model and authorize your own resources on Desktop before sharing this Bot with a Team.")
        Text("Listing: $listingId")
        handoff.versionId?.let { Text("Release: $it") }
        Button(enabled = !busy, onClick = { runCatching { handoff.save(context); LocalAppBrowser.open(context, handoff.marketUrl) }.onFailure { error = it.message } }) { Text("View in market") }
        Button(enabled = !busy, onClick = { scope.launch { busy = true; try {
            handoff = handoff.resolve(context, true)
            val share = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, handoff.desktopUrl)
            context.startActivity(Intent.createChooser(share, "Send to Desktop")); error = null
        } catch (failure: Exception) { error = failure.message } finally { busy = false } } }) { Text("Send to Desktop") }
        if (handoff.intentId != null) Button(enabled = !busy, onClick = { scope.launch { refresh() } }) { Text("Check Desktop download") }
        Text(if (handoff.status == "installed") "Desktop confirmed the personal Bot was created. Enable and share it there. No worker runs on this phone." else "Handoff saved. This page checks the authenticated Desktop receipt when you return. No Bot is installed on this phone.", style = MaterialTheme.typography.bodySmall)
        error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
    }
}
