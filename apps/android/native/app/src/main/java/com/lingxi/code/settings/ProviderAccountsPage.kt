package com.lingxi.code.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.unit.dp
import androidx.compose.ui.res.stringResource
import com.lingxi.code.R
import com.lingxi.code.model.LlmProviderCatalogEntry

/** The account settings route uses the same account authority as each provider editor. */
@Composable
internal fun ProviderAccountsPage(
    bridge: SettingsEngineBridge,
    repository: ProviderAccountsRepository,
    providers: List<LlmProviderCatalogEntry>,
    onChanged: () -> Unit,
    onMobileProfiles: () -> Unit,
) {
    val engine by bridge.state.collectAsState()
    val accounts by repository.state.collectAsState()
    var selected by remember(engine.generation) { mutableStateOf<String?>(null) }
    LaunchedEffect(engine.generation, accounts.bindingGeneration) { if (engine.connected) repository.refresh() }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        TextButton(onClick = onMobileProfiles) { Text(stringResource(R.string.provider_account_provider_configuration)) }
        if (!engine.connected) {
            Text(settingsLabel("Connect an engine to view and edit its configuration. Device settings remain available."))
            return@Column
        }
        val bindingIds = (providers.map { it.profileId } + accounts.accounts.map { it.executionBindingId }).distinct()
        val choices = bindingIds.filter { it != "openai-chatgpt" || "openai" !in bindingIds }
        choices.forEach { binding ->
            TextButton(onClick = { selected = binding }) {
                Text(providers.firstOrNull { it.profileId == binding }?.displayName ?: binding)
            }
        }
        val binding = selected?.takeIf { it in choices } ?: choices.firstOrNull()
        if (binding != null) ProviderAccountsSection(binding, repository, onChanged)
        else Text(accounts.error ?: stringResource(R.string.provider_accounts_loading))
    }
}
