package com.lingxi.code.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.lingxi.code.R
import com.lingxi.code.components.ModelDetailsDialog
import com.lingxi.code.components.ModelDetailsInfoButton
import com.lingxi.code.components.ModelSelectionSheet
import com.lingxi.code.model.CatalogModelDetails
import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderModelVisibility
import com.lingxi.code.model.ProviderModelVisibilityRules
import com.lingxi.code.theme.LingXiTheme

/** One catalog, with separate single-select and visibility-management tasks. */
@Composable
internal fun ProviderModelSheet(
    provider: GenericProvider,
    models: List<CatalogModelDetails>,
    catalogLoaded: Boolean,
    manageVisibility: Boolean,
    onUpdate: (GenericProvider) -> Unit,
    onSelect: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val t = LingXiTheme.palette
    var query by remember { mutableStateOf("") }
    var customExpanded by remember { mutableStateOf(models.isEmpty()) }
    var details by remember { mutableStateOf<CatalogModelDetails?>(null) }
    val matches = remember(models, query) {
        models.filter { query.isBlank() || it.modelId.contains(query.trim(), ignoreCase = true) || it.displayName.contains(query.trim(), ignoreCase = true) }
    }
    val selectedIds = ProviderModelVisibilityRules.selectedCatalogModelIds(
        models.map { it.modelId }, ProviderModelVisibility(showInModelPicker = provider.showInModelPicker, visibleModelIds = provider.visibleModelIds),
    )
    ModelSelectionSheet(
        title = stringResource(if (manageVisibility) R.string.settings_provider_visible_models else R.string.settings_provider_default_model),
        onDismiss = onDismiss,
    ) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text(stringResource(R.string.composer_search_models_placeholder)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp).testTag("provider.models.search"),
        )
        LazyColumn(Modifier.weight(1f).then(if (manageVisibility) Modifier else Modifier.selectableGroup())) {
            if (manageVisibility) {
                item {
                    ListItem(
                        headlineContent = { Text(stringResource(R.string.settings_provider_show_in_picker)) },
                        trailingContent = { Switch(checked = provider.showInModelPicker, onCheckedChange = null) },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = Modifier.toggleable(provider.showInModelPicker, role = Role.Switch) {
                            onUpdate(provider.copy(showInModelPicker = it))
                        }.testTag("provider.models.visible"),
                    )
                }
            } else {
                item {
                    TextButton(onClick = { customExpanded = !customExpanded }, modifier = Modifier.padding(horizontal = 12.dp)) {
                        Text(stringResource(R.string.settings_provider_custom_model_id))
                    }
                    if (customExpanded) {
                        Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp)) {
                            OutlinedTextField(
                                value = provider.model,
                                onValueChange = onSelect,
                                keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                                    keyboardType = androidx.compose.ui.text.input.KeyboardType.Ascii,
                                ),
                                label = { Text(stringResource(R.string.settings_provider_model_id)) },
                                singleLine = true,
                                modifier = Modifier.fillMaxWidth().testTag("provider.models.custom"),
                            )
                            Text(stringResource(R.string.settings_provider_custom_model_hint), style = MaterialTheme.typography.bodySmall, color = t.text3)
                        }
                    }
                }
            }
            if (models.isEmpty()) {
                item {
                    Text(
                        stringResource(if (catalogLoaded) R.string.settings_provider_model_catalog_unavailable else R.string.settings_provider_catalog_loading),
                        color = t.text3, modifier = Modifier.padding(24.dp),
                    )
                }
            } else if (matches.isEmpty()) {
                item { Text(stringResource(R.string.composer_no_matching_models), color = t.text3, modifier = Modifier.padding(24.dp)) }
            }
            if (!manageVisibility || provider.showInModelPicker) {
                items(matches, key = { it.modelId }) { model ->
                    val selected = if (manageVisibility) model.modelId in selectedIds else model.modelId == provider.model
                    val select = {
                        if (manageVisibility) {
                            val base = provider.visibleModelIds ?: models.map { it.modelId }
                            onUpdate(provider.copy(visibleModelIds = if (selected) base - model.modelId else (base + model.modelId).distinct()))
                        } else onSelect(model.modelId)
                    }
                    ListItem(
                        headlineContent = { Text(model.displayName) },
                        supportingContent = if (model.displayName != model.modelId) {{ Text(model.modelId) }} else null,
                        leadingContent = {
                            if (manageVisibility) Checkbox(checked = selected, onCheckedChange = null)
                            else RadioButton(selected = selected, onClick = null)
                        },
                        trailingContent = { ModelDetailsInfoButton(onClick = { details = model }) },
                        colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                        modifier = (if (manageVisibility) Modifier.toggleable(selected, role = Role.Checkbox) { select() }
                            else Modifier.selectable(selected, role = Role.RadioButton, onClick = select))
                            .testTag("provider.models.row.${model.modelId}"),
                    )
                }
            }
            if (manageVisibility && provider.showInModelPicker && selectedIds.isEmpty()) {
                item { Text(stringResource(R.string.settings_provider_picker_hidden_empty), color = t.statusTesting, modifier = Modifier.padding(24.dp)) }
            }
        }
    }
    details?.let { ModelDetailsDialog(it, onDismiss = { details = null }) }
}
