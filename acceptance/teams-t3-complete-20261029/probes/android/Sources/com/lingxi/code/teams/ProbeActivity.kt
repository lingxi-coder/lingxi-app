package com.lingxi.code.teams

import android.app.Application
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.viewmodel.CreationExtras
import androidx.lifecycle.viewmodel.compose.viewModel
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.auth.FirebaseAuth
import com.google.firebase.firestore.FirebaseFirestore
import com.google.firebase.firestore.FirebaseFirestoreSettings
import com.google.firebase.firestore.MemoryCacheSettings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/** This application exists only in the demo feature probe APK. */
class ProbeApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        FirebaseApp.initializeApp(this, FirebaseOptions.Builder().setProjectId("demo-lingxi-teams-t0")
            .setApplicationId("1:1234567890:android:0000000000000000").setApiKey("demo-emulator-only-key").build())
        FirebaseAuth.getInstance().apply { useEmulator("10.0.2.2", 9098); signOut() }
        FirebaseFirestore.getInstance().apply {
            firestoreSettings = FirebaseFirestoreSettings.Builder().setLocalCacheSettings(MemoryCacheSettings.newBuilder().build()).build()
            useEmulator("10.0.2.2", 8085)
        }
    }
}

class ProbeActivity : ComponentActivity() {
    private var fixture by mutableStateOf<JSONObject?>(null)
    private var failure by mutableStateOf("")
    internal var messagesModel: TeamMessagingModel? = null
    internal val apiRoot: String get() {
        val port = intent.getIntExtra("fixture_port", 18783)
        check(port in 1024..65535)
        return "http://10.0.2.2:$port"
    }
    override val defaultViewModelProviderFactory: ViewModelProvider.Factory
        get() {
            val fallback = super.defaultViewModelProviderFactory
            return object : ViewModelProvider.Factory {
                @Suppress("UNCHECKED_CAST")
                override fun <T : ViewModel> create(modelClass: Class<T>, extras: CreationExtras): T = when (modelClass) {
                    TeamMessagingModel::class.java -> TeamMessagingModel(application, apiRoot) as T
                    TeamsViewModel::class.java -> TeamsViewModel(apiRoot) as T
                    else -> fallback.create(modelClass, extras)
                }
            }
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lifecycleScope.launch {
            try {
                val uid = intent.getStringExtra("fixture_uid") ?: "android-human"
                require(uid.isNotBlank())
                val value = withContext(Dispatchers.IO) { request("GET", "/_acceptance/bootstrap?uid=${URLEncoder.encode(uid, "UTF-8")}") }
                FirebaseAuth.getInstance().signInWithCustomToken(value.getString("customToken")).await()
                check(FirebaseAuth.getInstance().currentUser?.uid == uid)
                fixture = value
            } catch (error: Exception) { failure = "${error.javaClass.simpleName}: ${error.message}" }
        }
        setContent {
            MaterialTheme {
                val value = fixture
                var home by remember { mutableStateOf(intent.getStringExtra("page") == "home") }
                Column(Modifier.fillMaxSize()) {
                    Text("Demo emulator feature harness · not full product", Modifier.padding(8.dp), style = MaterialTheme.typography.labelSmall)
                    if (failure.isNotEmpty()) Text("HARNESS ERROR: $failure", color = MaterialTheme.colorScheme.error)
                    else if (value == null) Text("Signing in to demo emulators…")
                    else {
                        val detail = TeamsViewModel.detail(value.getJSONObject("detail"))
                        val uid = requireNotNull(FirebaseAuth.getInstance().currentUser?.uid)
                        Box(Modifier.weight(1f)) {
                            if (home) TeamsScreen(null, {}, {}, { home = false })
                            else {
                                val model: TeamMessagingModel = viewModel(key = "teams-messages:$uid:${detail.team.id}:${detail.team.generation}")
                                messagesModel = model
                                TeamMessagesScreen(uid, detail, { home = true }, model)
                                val conversation = intent.getStringExtra("conversation_id")
                                LaunchedEffect(model, conversation) { if (!conversation.isNullOrEmpty()) model.select(conversation) }
                            }
                        }
                    }
                }
            }
        }
    }

    /** Only the separate demo APK has this local router; product never reads Intents for endpoints. */
    internal fun request(method: String, path: String, body: JSONObject? = null, token: String? = null): JSONObject {
        val connection = URL(apiRoot + path).openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 10000; connection.readTimeout = 10000; connection.instanceFollowRedirects = false
            connection.requestMethod = method
            token?.let { connection.setRequestProperty("Authorization", "Bearer $it") }
            if (body != null) {
                connection.doOutput = true; connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            }
            val success = connection.responseCode in 200..299
            val stream = if (success) connection.inputStream else connection.errorStream
            val result = JSONObject(stream?.bufferedReader()?.use { it.readText() } ?: "{}")
            check(success) { "Fixture HTTP ${connection.responseCode}: ${result.optJSONObject("error")?.optString("code")}" }
            return result
        } finally { connection.disconnect() }
    }
}
