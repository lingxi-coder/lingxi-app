package com.lingxi.code.settings

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import com.lingxi.code.R
import com.lingxi.code.theme.LingXiTheme
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.text.DateFormat
import java.util.Date

/** Official credential pages; custom providers intentionally have no inferred destination. */
private fun providerCredentialManagementUrl(providerId: String): String? = when (providerId) {
    "anthropic" -> "https://console.anthropic.com/settings/keys"
    "openai" -> "https://platform.openai.com/api-keys"
    "xai" -> "https://console.x.ai"
    "deepseek" -> "https://platform.deepseek.com/api_keys"
    "kimi" -> "https://platform.kimi.com/console/api-keys"
    "kimi-code" -> "https://www.kimi.com/code/console"
    "gemini" -> "https://aistudio.google.com/app/apikey"
    "openrouter" -> "https://openrouter.ai/settings/keys"
    "zai" -> "https://z.ai/manage-apikey/apikey-list"
    "glm-coding" -> "https://bigmodel.cn/usercenter/proj-mgmt/apikeys"
    "github-copilot" -> "https://github.com/settings/tokens"
    else -> null
}

/** Stable user order is rendered directly; the Host owns temporary health and request scheduling. */
@Composable
internal fun ProviderAccountsSection(bindingId: String, repository: ProviderAccountsRepository, onChanged: () -> Unit, canAddAccounts: Boolean = true, providerName: String = bindingId) {
    val context = LocalContext.current
    val state by repository.state.collectAsState()
    val scope = rememberCoroutineScope()
    val lifecycleOwner = LocalLifecycleOwner.current
    val logicalProviderId = if (bindingId == "openai-chatgpt") "openai" else bindingId
    val accounts = state.accounts.filter { it.providerId == logicalProviderId }
    var busy by remember(bindingId) { mutableStateOf(false) }
    var message by remember(bindingId) { mutableStateOf<String?>(null) }
    var messageIsError by remember(bindingId) { mutableStateOf(true) }
    var edit by remember(bindingId) { mutableStateOf<ProviderAccountRow?>(null) }
    var showAccountOptions by remember(bindingId) { mutableStateOf(false) }
    var adding by remember(bindingId) { mutableStateOf(false) }
    var oauth by remember(bindingId) { mutableStateOf(false) }
    var confirmRemove by remember(bindingId) { mutableStateOf<ProviderAccountRow?>(null) }
    var modelsExpanded by remember(bindingId) { mutableStateOf(false) }
    var confirmMetered by remember(bindingId) { mutableStateOf(false) }
    var name by remember(bindingId) { mutableStateOf("") }
    var key by remember(bindingId) { mutableStateOf("") }
    var loginJob by remember(bindingId) { mutableStateOf<Job?>(null) }
    var deviceAuthorization by remember(bindingId, state.bindingGeneration) { mutableStateOf<ProviderDeviceAuthorization?>(null) }
    val palette = LingXiTheme.palette
    ReportSettingsDraft("provider-account-$bindingId", key.isNotEmpty() || adding || edit != null)
    LaunchedEffect(bindingId, state.bindingGeneration, lifecycleOwner) {
        edit = null; adding = false; key = ""; confirmMetered = false; confirmRemove = null
        lifecycleOwner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            while (true) {
                repository.refresh()
                delay(15_000)
            }
        }
    }
    fun execute(action: String, fields: JSONObject, credential: String? = null, after: (() -> Unit)? = null) {
        busy = true
        message = null
        messageIsError = true
        scope.launch {
            try {
                message = repository.command(action, fields, credential)
                if (message == null) { onChanged(); after?.invoke() }
            } finally { busy = false }
        }
    }
    SettingsSection(label = stringResource(R.string.provider_accounts_title), footer = if (accounts.size > 1) stringResource(R.string.provider_accounts_order_hint) else null) {
        if (!state.loaded) Text(state.error ?: stringResource(R.string.provider_accounts_loading), color = palette.text3)
        if (state.loaded && accounts.isEmpty()) Text(stringResource(R.string.provider_accounts_empty), color = palette.text3)
        if (state.allAccountsUnavailable) Text(stringResource(R.string.provider_account_all_unavailable), color = palette.danger)
        accounts.forEachIndexed { index, account ->
            Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                SettingsRow(
                    label = "${index + 1}. ${account.displayName}",
                    sub = stringResource(when {
                        account.authKind == "api_key" -> R.string.provider_account_auth_api_key
                        account.executionBindingId == "openai-chatgpt" -> R.string.provider_account_auth_chatgpt
                        else -> R.string.provider_account_auth_oauth
                    }),
                    chevron = false,
                    trailing = { com.lingxi.code.components.LXToggle(checked = account.enabled, enabled = !busy,
                        onCheckedChange = { execute("set_enabled", JSONObject().put("accountId", account.accountId).put("enabled", it)) }) },
                )
                if (state.lastSelectedAccountId == account.accountId) Text(
                    stringResource(R.string.provider_account_last_selected) + state.lastSelectedModel?.let { " · $it" }.orEmpty(),
                    color = palette.accent, modifier = Modifier.padding(horizontal = 12.dp),
                )
                val statusLabel = when (account.status) {
                    "ready" -> stringResource(R.string.provider_account_ready)
                    "needs_auth" -> stringResource(R.string.provider_account_needs_auth)
                    "cooling_down" -> stringResource(R.string.provider_account_cooldown)
                    "quota_exhausted" -> stringResource(R.string.provider_account_quota)
                    "disabled" -> stringResource(R.string.provider_account_disabled)
                    "storage_unavailable" -> stringResource(R.string.provider_account_storage_unavailable)
                    else -> stringResource(R.string.provider_account_status_unknown)
                }
                Text(statusLabel + (account.cooldownUntilMs?.let { " · ${DateFormat.getDateTimeInstance().format(Date(it))}" } ?: ""),
                    color = palette.text3, modifier = Modifier.padding(horizontal = 12.dp))
                state.scopedHealth.filter { it.accountId == account.accountId && it.identityGeneration == account.identityGeneration }.forEach { health ->
                    val scopeLabel = stringResource(if (health.scopeKind == "model") R.string.provider_account_scope_model else R.string.provider_account_scope_endpoint)
                    val reasonLabel = stringResource(when (health.reason) {
                        "rate_limited" -> R.string.provider_account_rate_limited
                        "quota_exhausted" -> R.string.provider_account_quota
                        "model_permission_denied" -> R.string.provider_account_model_forbidden
                        "authentication_rejected", "credential_missing", "authentication_required" -> R.string.provider_account_needs_auth
                        else -> R.string.provider_account_temporarily_unavailable
                    })
                    Text(stringResource(R.string.provider_account_scoped_health, scopeLabel, health.scopeId, reasonLabel) +
                        health.unavailableUntilMs?.let { " · ${DateFormat.getDateTimeInstance().format(Date(it))}" }.orEmpty(),
                        color = palette.text3, modifier = Modifier.padding(horizontal = 12.dp))
                }
                var permissionsExpanded by remember(account.accountId) { mutableStateOf(false) }
                val entitlements = state.modelEntitlements.filter { it.accountId == account.accountId && it.identityGeneration == account.identityGeneration }.sortedBy { it.modelId }
                if (entitlements.isNotEmpty()) TextButton(onClick = { permissionsExpanded = !permissionsExpanded }) {
                    Text(stringResource(R.string.provider_account_permissions_count, entitlements.size))
                }
                if (permissionsExpanded) entitlements.forEach { entitlement ->
                    Text(stringResource(R.string.provider_account_model_observed, entitlement.modelId, accountModelPermissionLabel(entitlement.status)) +
                        entitlement.retryAtMs?.let { " · ${DateFormat.getDateTimeInstance().format(Date(it))}" }.orEmpty(),
                        color = palette.text3, modifier = Modifier.padding(horizontal = 12.dp))
                }
                Row {
                    TextButton(enabled = !busy, onClick = { edit = account; name = account.displayName; key = ""; adding = false; oauth = account.authKind == "oauth" }) {
                        Text(stringResource(R.string.provider_account_edit))
                    }
                    TextButton(enabled = !busy && index > 0, onClick = {
                        val ids = accounts.map { it.accountId }.toMutableList()
                        java.util.Collections.swap(ids, index, index - 1)
                        execute("reorder", JSONObject().put("providerId", logicalProviderId).put("accountIds", JSONArray(ids)))
                    }) { Text(stringResource(R.string.provider_account_up)) }
                    TextButton(enabled = !busy && index < accounts.lastIndex, onClick = {
                        val ids = accounts.map { it.accountId }.toMutableList()
                        java.util.Collections.swap(ids, index, index + 1)
                        execute("reorder", JSONObject().put("providerId", logicalProviderId).put("accountIds", JSONArray(ids)))
                    }) { Text(stringResource(R.string.provider_account_down)) }
                }
                Row {
                    TextButton(enabled = !busy, onClick = { execute("test", JSONObject().put("accountId", account.accountId)) { message = context.getString(R.string.provider_account_test_success); messageIsError = false } }) {
                        Text(stringResource(R.string.settings_provider_test_connection))
                    }
                    if (account.authKind == "oauth" && state.oauthSupported) TextButton(enabled = !busy, onClick = {
                        busy = true
                        messageIsError = true
                        message = null
                        loginJob = scope.launch {
                            try {
                                message = repository.login(account) { deviceAuthorization = it }
                                if (message == null) onChanged()
                            } finally { busy = false; loginJob = null; deviceAuthorization = null }
                        }
                    }) { Text(stringResource(R.string.provider_account_signin)) }
                }
            }
        }
        val modelRows = state.modelAvailability.filter { it.providerId == logicalProviderId }
            .sortedWith(compareBy<ProviderAccountModelAvailability> { it.modelId }.thenBy { it.executionBindingId })
        if (modelRows.isNotEmpty()) {
            TextButton(onClick = { modelsExpanded = !modelsExpanded }) {
                Text(stringResource(R.string.provider_account_models_union))
            }
            if (modelsExpanded) Text(stringResource(R.string.provider_account_models_verification_hint), color = palette.text3)
            if (modelsExpanded) modelRows.forEach { model ->
                val authLabel = stringResource(if (model.executionBindingId == "openai-chatgpt") R.string.provider_account_auth_chatgpt else R.string.provider_account_auth_api_key)
                val names = accounts.filter { it.accountId in model.eligibleAccountIds }.joinToString { it.displayName }
                Text("${model.modelId} · $authLabel · ${accountModelPermissionLabel(model.permissionStatus)}", color = palette.text)
                Text(names.ifEmpty { stringResource(R.string.provider_account_model_no_eligible) }, color = palette.text3)
            }
        }
        if (!canAddAccounts) Text(stringResource(R.string.provider_account_apply_configuration_first), color = palette.text3)
        Row {
            if (bindingId != "openai-chatgpt") TextButton(enabled = !busy && state.loaded && canAddAccounts, onClick = {
                adding = true; oauth = false; edit = null; name = ""; key = ""; showAccountOptions = false
            }) { Text(stringResource(R.string.provider_account_add_key)) }
            if (logicalProviderId == "openai" && state.oauthSupported) TextButton(enabled = !busy && state.loaded && canAddAccounts, onClick = {
                adding = true; oauth = true; edit = null; name = ""; key = ""; showAccountOptions = false
            }) { Text(stringResource(R.string.provider_account_add_oauth)) }
        }
        if (!state.oauthSupported && logicalProviderId == "openai") Text(stringResource(R.string.provider_account_remote_oauth_hint), color = palette.text3)
        if (logicalProviderId == "openai") {
            SettingsRow(
                label = stringResource(R.string.provider_account_allow_metered),
                sub = stringResource(R.string.provider_account_metered_fee_hint),
                chevron = false,
                trailing = {
                    com.lingxi.code.components.LXToggle(
                        checked = state.providerPolicies[logicalProviderId]?.allowSubscriptionToMetered == true,
                        enabled = !busy && state.loaded,
                        onCheckedChange = { allow ->
                            if (allow) confirmMetered = true
                            else execute("set_policy", JSONObject().put("providerId", logicalProviderId).put("allowSubscriptionToMetered", false))
                        },
                    )
                },
            )
        }
        deviceAuthorization?.let { authorization ->
            val displayName = accounts.firstOrNull { it.accountId == authorization.accountId }?.displayName.orEmpty()
            Text(stringResource(R.string.provider_account_device_instructions, displayName), color = palette.text)
            SelectionContainer { Text(authorization.userCode, color = palette.accent) }
            Text(authorization.verificationUrl, color = palette.text3)
            Text(stringResource(R.string.provider_account_device_enable_hint), color = palette.text3)
            TextButton(onClick = {
                runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(authorization.verificationUrl)).addCategory(Intent.CATEGORY_BROWSABLE)) }
                    .onFailure { message = context.getString(R.string.provider_account_device_browser_failed); messageIsError = true }
            }) { Text(stringResource(R.string.provider_account_device_open_browser)) }
        }
        if (loginJob != null) TextButton(onClick = { loginJob?.cancel() }) { Text(stringResource(R.string.provider_account_cancel_signin)) }
        if (message != null || state.error != null) Text(message ?: state.error.orEmpty(), color = if (message == null || messageIsError) palette.danger else palette.text3, modifier = Modifier.padding(12.dp))
        TextButton(enabled = !busy, onClick = { scope.launch { repository.refresh() } }) { Text(stringResource(R.string.provider_accounts_refresh)) }
    }
    confirmRemove?.let { account ->
        AlertDialog(
            onDismissRequest = { confirmRemove = null },
            title = { Text(stringResource(R.string.provider_account_remove)) },
            text = { Text(stringResource(R.string.provider_account_remove_confirmation, account.displayName)) },
            confirmButton = { TextButton(enabled = !busy, onClick = {
                confirmRemove = null
                execute("remove", JSONObject().put("accountId", account.accountId)) { edit = null; key = "" }
            }) { Text(stringResource(R.string.provider_account_remove), color = palette.danger) } },
            dismissButton = { TextButton(onClick = { confirmRemove = null }) { Text(stringResource(android.R.string.cancel)) } },
        )
    }
    if (confirmMetered) AlertDialog(
        onDismissRequest = { confirmMetered = false },
        title = { Text(stringResource(R.string.provider_account_allow_metered)) },
        text = { Text(stringResource(R.string.provider_account_metered_consent)) },
        confirmButton = { TextButton(onClick = {
            confirmMetered = false
            execute("set_policy", JSONObject().put("providerId", logicalProviderId).put("allowSubscriptionToMetered", true))
        }) { Text(stringResource(R.string.provider_account_accept_api_fees)) } },
        dismissButton = { TextButton(onClick = { confirmMetered = false }) { Text(stringResource(android.R.string.cancel)) } },
    )
    if (adding || edit != null) {
        AlertDialog(
            onDismissRequest = { if (!busy) { adding = false; edit = null; key = "" } },
            title = { Text(stringResource(if (adding) R.string.provider_account_add_title else R.string.provider_account_edit)) },
            text = {
                Column(verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(8.dp)) {
                    if (adding) Text(providerName, color = palette.text3)
                    if (busy) androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth())
                    if (message != null) Text(message.orEmpty(), color = palette.danger)
                    if (!adding) OutlinedTextField(value = name, onValueChange = { name = it }, enabled = !busy, singleLine = true,
                        label = { Text(stringResource(R.string.settings_display_name)) })
                    if (!oauth) OutlinedTextField(value = key, onValueChange = { key = it }, enabled = !busy, singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        label = { Text(stringResource(R.string.settings_provider_api_key)) })
                    if (!oauth) providerCredentialManagementUrl(bindingId)?.let { url ->
                        TextButton(onClick = {
                            runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)) }
                                .onFailure { message = context.getString(R.string.provider_account_device_browser_failed); messageIsError = true }
                        }) { Text(stringResource(R.string.provider_account_manage_api_key)) }
                    }
                    if (adding) TextButton(enabled = !busy, onClick = { showAccountOptions = !showAccountOptions }) {
                        Text(stringResource(R.string.settings_section_advanced))
                    }
                    if (adding && showAccountOptions) OutlinedTextField(
                        value = name, onValueChange = { name = it }, enabled = !busy, singleLine = true,
                        placeholder = { Text(providerName) },
                        label = { Text(stringResource(R.string.provider_account_optional_name)) },
                    )
                    edit?.let { account ->
                        TextButton(enabled = !busy, onClick = {
                            execute("clear_credential", JSONObject().put("accountId", account.accountId)) { edit = null; key = "" }
                        }) { Text(stringResource(if (oauth) R.string.provider_account_logout else R.string.settings_provider_clear_key)) }
                        TextButton(enabled = !busy, onClick = {
                            confirmRemove = account
                        }) { Text(stringResource(R.string.provider_account_remove), color = palette.danger) }
                    }
                }
            },
            confirmButton = { TextButton(enabled = !busy && (adding || name.isNotBlank()) && (!adding || oauth || key.isNotBlank()), onClick = {
                val account = edit
                if (account == null) {
                    val fields = JSONObject().put("displayName", name.trim().ifEmpty { providerName }).put("providerId", logicalProviderId)
                        .put("executionBindingId", if (oauth) "openai-chatgpt" else bindingId).put("authKind", if (oauth) "oauth" else "api_key")
                    execute("add", fields, key.takeIf { it.isNotBlank() }) { adding = false; edit = null; key = "" }
                } else {
                    busy = true
                    messageIsError = true
                    scope.launch {
                        try {
                            message = repository.saveAccount(account, name.trim(), key.takeIf { it.isNotBlank() })
                            if (message == null) { edit = null; key = ""; onChanged() }
                        } finally { busy = false }
                    }
                }
            }) { Text(stringResource(if (adding) { if (oauth) R.string.provider_account_add_oauth else R.string.provider_account_add_key } else R.string.voice_save_button)) } },
            dismissButton = { TextButton(enabled = !busy, onClick = { adding = false; edit = null; key = "" }) { Text(stringResource(android.R.string.cancel)) } },
        )
    }
}

@Composable
internal fun accountModelPermissionLabel(status: String): String = stringResource(when (status) {
    "confirmed" -> R.string.provider_account_model_confirmed
    "forbidden" -> R.string.provider_account_model_forbidden
    else -> R.string.provider_account_model_unverified
})

@Composable
internal fun accountAvailabilityProblemLabel(problem: ProviderAccountAvailabilityProblem): String = stringResource(when (problem) {
    ProviderAccountAvailabilityProblem.CoolingDown -> R.string.provider_account_unavailable_cooldown
    ProviderAccountAvailabilityProblem.NeedsAuthentication -> R.string.provider_account_unavailable_auth
    ProviderAccountAvailabilityProblem.QuotaExhausted -> R.string.provider_account_unavailable_quota
    ProviderAccountAvailabilityProblem.Disabled -> R.string.provider_account_unavailable_disabled
    ProviderAccountAvailabilityProblem.StorageUnavailable -> R.string.provider_account_unavailable_storage
    ProviderAccountAvailabilityProblem.ModelForbidden -> R.string.provider_account_unavailable_model
    ProviderAccountAvailabilityProblem.NoEligibleAccount -> R.string.provider_account_unavailable_generic
})

@Composable
internal fun providerConfigurationLabel(state: ProviderAccountsState, bindingId: String): String {
    val status = stringResource(when (state.configurationStatus(bindingId)) {
        "read_failed" -> R.string.provider_accounts_read_failed
        "loading" -> R.string.provider_accounts_loading
        "unconfigured" -> R.string.settings_provider_unconfigured
        "configured" -> R.string.settings_provider_status_configured
        "disabled" -> R.string.provider_account_disabled
        "needs_auth" -> R.string.provider_account_needs_auth
        "temporarily_unavailable" -> R.string.provider_account_temporarily_unavailable
        "storage_unavailable" -> R.string.provider_account_storage_unavailable
        else -> R.string.provider_account_status_unknown
    })
    val accounts = state.providerAccounts(bindingId)
    return if (accounts.isEmpty()) status else "$status · ${accounts.count { it.enabled }}/${accounts.size}"
}
