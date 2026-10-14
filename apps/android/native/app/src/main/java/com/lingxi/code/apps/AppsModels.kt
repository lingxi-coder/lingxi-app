package com.lingxi.code.apps

import com.lingxi.code.market.InstalledApp
import com.lingxi.code.market.MarketOperation
import com.lingxi.code.market.MarketUpdate
import com.lingxi.code.market.MarketVersion
import com.lingxi.code.market.OperationPhase
import com.lingxi.code.market.UpdateStatus

/** One installed app as the 应用 page and the drawer show it. */
data class MyApp(
    val appId: String,
    val name: String,
    val category: String,
    val lastOpenedAtMillis: Long?,
    val updateAvailable: Boolean,
    /** Only a download, install or uninstall that is still running. */
    val activeOperation: MarketOperation?,
)

private val ActivePhases = setOf(OperationPhase.DOWNLOADING, OperationPhase.COMMITTING, OperationPhase.UNINSTALLING)

/**
 * The installed apps, most recently opened first, then by name. An app the
 * pages know nothing about yet is shown by its id until its details load.
 * PURE, so the order is unit-testable on the plain JVM.
 */
internal fun myApps(
    installed: List<InstalledApp>,
    info: Map<String, InstalledAppInfo>,
    updates: List<MarketUpdate>,
    operations: Map<String, MarketOperation>,
): List<MyApp> {
    val updatable = updates.filter { it.status == UpdateStatus.UPDATE_AVAILABLE }.mapTo(HashSet()) { it.appId }
    return installed
        .map { record ->
            val known = info[record.appId]
            MyApp(
                appId = record.appId,
                name = known?.name?.takeIf(String::isNotBlank) ?: record.appId,
                category = known?.category.orEmpty(),
                lastOpenedAtMillis = known?.lastOpenedAtMillis,
                updateAvailable = record.appId in updatable,
                activeOperation = operations[record.appId]?.takeIf { it.phase in ActivePhases },
            )
        }
        .sortedWith(
            compareByDescending<MyApp> { it.lastOpenedAtMillis ?: Long.MIN_VALUE }
                .thenBy(String.CASE_INSENSITIVE_ORDER) { it.name }
                .thenBy { it.appId },
        )
}

/** The apps shown on the drawer's 应用 row: the last few that were actually opened. */
internal fun recentApps(apps: List<MyApp>, limit: Int = 3): List<MyApp> =
    apps.filter { it.lastOpenedAtMillis != null }.sortedByDescending { it.lastOpenedAtMillis }.take(limit)

internal fun matchingApps(apps: List<MyApp>, query: String): List<MyApp> {
    val needle = query.trim()
    if (needle.isEmpty()) return emptyList()
    return apps.filter { it.name.contains(needle, ignoreCase = true) }
}

/** The single character of an app's cover, like the market's own covers. */
internal fun appInitial(name: String): String {
    val trimmed = name.trim()
    if (trimmed.isEmpty()) return "?"
    return String(Character.toChars(trimmed.codePointAt(0))).uppercase()
}

/** What an app's main button does right now. */
sealed interface AppAction {
    data object Get : AppAction
    data object Open : AppAction
    data class Update(val versionId: String) : AppAction
    /** A download in flight; tapping cancels it. [fraction] is null until the size is known. */
    data class Downloading(val fraction: Float?) : AppAction
    data object Installing : AppAction
    data object Removing : AppAction
    /** Not installed and the market has no version to install. */
    data object Unavailable : AppAction
}

/**
 * The main button for one app, from what the market reported last. A newer
 * version counts when the update check said so or when the listing's current
 * version is past the installed one.
 */
internal fun appAction(
    currentVersion: MarketVersion?,
    installed: InstalledApp?,
    update: MarketUpdate?,
    operation: MarketOperation?,
): AppAction {
    when (operation?.phase) {
        OperationPhase.DOWNLOADING -> return AppAction.Downloading(
            fraction = if (operation.totalBytes > 0) {
                (operation.downloadedBytes.toFloat() / operation.totalBytes).coerceIn(0f, 1f)
            } else {
                null
            },
        )
        OperationPhase.COMMITTING -> return AppAction.Installing
        OperationPhase.UNINSTALLING -> return AppAction.Removing
        else -> Unit
    }
    if (installed == null) return if (currentVersion != null) AppAction.Get else AppAction.Unavailable
    val checked = update?.takeIf { it.status == UpdateStatus.UPDATE_AVAILABLE }?.currentVersionId
    val listed = currentVersion?.takeIf { it.sequence > installed.sequence }?.id
    return (checked ?: listed)?.let(AppAction::Update) ?: AppAction.Open
}

/** The error code of a failed operation, for the line under the main button. */
internal fun failedOperationCode(operation: MarketOperation?): String? =
    operation?.takeIf { it.phase == OperationPhase.FAILED }?.let { it.error ?: "UNKNOWN_ERROR" }
