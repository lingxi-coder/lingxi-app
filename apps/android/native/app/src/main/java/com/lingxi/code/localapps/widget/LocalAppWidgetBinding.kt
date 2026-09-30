package com.lingxi.code.localapps.widget

internal data class LocalAppWidgetPinBind(
    val appId: String,
    val widgetId: Int,
)

internal fun resolvePinnedWidgetBind(appId: String?, widgetId: Int): LocalAppWidgetPinBind? {
    val validAppId = appId?.takeIf(LocalAppWidgetDeepLink::isValidAppId) ?: return null
    if (widgetId == android.appwidget.AppWidgetManager.INVALID_APPWIDGET_ID) return null
    return LocalAppWidgetPinBind(validAppId, widgetId)
}

internal enum class WidgetOwnership {
    Accept,
    Wait,
    Reject,
}

internal fun widgetOwnership(widgetId: Int, ownedWidgetIds: IntArray): WidgetOwnership {
    if (widgetId == android.appwidget.AppWidgetManager.INVALID_APPWIDGET_ID) return WidgetOwnership.Reject
    if (widgetId in ownedWidgetIds) return WidgetOwnership.Accept
    // A just-allocated id can be missing from getAppWidgetIds even when other
    // widgets are already registered. Only INVALID is a hard reject.
    return WidgetOwnership.Wait
}

internal fun canConfigureWidget(widgetId: Int, ownedWidgetIds: IntArray): Boolean =
    widgetOwnership(widgetId, ownedWidgetIds) == WidgetOwnership.Accept

// After a pin callback writes a binding for a not-yet-listed id, retract it
// if the id never becomes owned. Otherwise a late callback after delete can
// leave a store entry that auto-binds the next recycled widget id.
internal fun shouldRetractPinBinding(
    ownership: WidgetOwnership,
    storedAppId: String?,
    writtenAppId: String,
): Boolean {
    if (ownership == WidgetOwnership.Accept) return false
    return storedAppId != null &&
        storedAppId == writtenAppId &&
        LocalAppWidgetDeepLink.isValidAppId(writtenAppId)
}

internal enum class ConfigureAction {
    AutoBind,
    ShowPicker,
    Retry,
    Finish,
}

internal data class ConfigureStep(
    val action: ConfigureAction,
    val appId: String? = null,
)

// Exported configure must not show the picker, or write a binding, until
// this provider owns the widget id. Wait is a short registration race only.
internal fun decideConfigureStep(
    ownership: WidgetOwnership,
    existingAppId: String?,
    widgetScopedAppId: String? = null,
    retriesRemaining: Boolean,
): ConfigureStep {
    return when (ownership) {
        WidgetOwnership.Reject -> ConfigureStep(ConfigureAction.Finish)
        WidgetOwnership.Wait ->
            if (retriesRemaining) {
                ConfigureStep(ConfigureAction.Retry)
            } else {
                ConfigureStep(ConfigureAction.Finish)
            }
        WidgetOwnership.Accept -> {
            val appId = existingAppId?.takeIf(LocalAppWidgetDeepLink::isValidAppId)
                ?: widgetScopedAppId?.takeIf(LocalAppWidgetDeepLink::isValidAppId)
            if (appId != null) {
                ConfigureStep(ConfigureAction.AutoBind, appId)
            } else {
                ConfigureStep(ConfigureAction.ShowPicker)
            }
        }
    }
}

internal fun resolveConfigureAppId(
    widgetId: Int,
    ownedWidgetIds: IntArray,
    existingAppId: String?,
    widgetScopedAppId: String? = null,
): String? {
    val step = decideConfigureStep(
        ownership = widgetOwnership(widgetId, ownedWidgetIds),
        existingAppId = existingAppId,
        widgetScopedAppId = widgetScopedAppId,
        retriesRemaining = false,
    )
    return step.appId.takeIf { step.action == ConfigureAction.AutoBind }
}

internal fun remappedWidgetBindings(
    oldIds: IntArray,
    newIds: IntArray,
    current: Map<Int, String>,
): Map<Int, String> {
    val count = minOf(oldIds.size, newIds.size)
    val moved = ArrayList<Pair<Int, String>>(count)
    for (index in 0 until count) {
        val appId = current[oldIds[index]] ?: continue
        moved += newIds[index] to appId
    }
    val next = current.toMutableMap()
    for (index in 0 until count) {
        next.remove(oldIds[index])
    }
    moved.forEach { (widgetId, appId) ->
        next[widgetId] = appId
    }
    return next
}
