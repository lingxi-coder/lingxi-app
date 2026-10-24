package com.lingxi.code.apps

/** A stable schema-2 installation as shown in the drawer. */
data class MyApp(val appId: String, val name: String, val category: String, val lastOpenedAtMillis: Long?)

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
