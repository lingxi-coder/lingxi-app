package com.lingxi.code.computeruse

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri

/** Use Android's actual web handlers, including browsers installed by device vendors. */
internal fun resolveBrowserPackages(context: Context, candidatePackages: Set<String>): Set<String> {
    val webIntent = Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com"))
        .addCategory(Intent.CATEGORY_BROWSABLE)
    return candidatePackages.filterTo(linkedSetOf()) { packageName ->
        runCatching {
            context.packageManager.resolveActivity(
                Intent(webIntent).setPackage(packageName),
                PackageManager.MATCH_DEFAULT_ONLY,
            ) != null
        }.getOrDefault(false)
    }
}

/** An active session's grants are authoritative; a saved choice grants no live access. */
internal fun browserControlTier(
    state: ComputerUseUiState,
    configuration: ComputerUseConfiguration,
    packageName: String,
): ComputerUseTier? = if (state.sessionState == ComputerUseSessionState.Inactive) {
    configuration.appSelections[packageName]
} else {
    state.grants.firstOrNull { it.packageName == packageName }?.tier
}

internal fun browserAppSelection(
    configuration: ComputerUseConfiguration,
    sessionState: ComputerUseSessionState,
    browserPackages: Set<String>,
    packageName: String,
    tier: ComputerUseTier?,
): ComputerUseConfiguration {
    require(packageName in browserPackages) { "Select an installed browser" }
    if (sessionState != ComputerUseSessionState.Inactive) return configuration
    val selections = configuration.appSelections.toMutableMap()
    if (tier == null) selections.remove(packageName) else selections[packageName] = tier
    return configuration.copy(appSelections = selections)
}
