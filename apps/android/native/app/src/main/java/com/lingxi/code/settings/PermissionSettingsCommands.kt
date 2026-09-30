package com.lingxi.code.settings

import com.lingxi.code.bindings.ClientCommand
import com.lingxi.code.bindings.PermissionBehaviorDto
import com.lingxi.code.bindings.WritableScopeDto
import org.json.JSONObject

/** Permissions is reserved by update_settings; edits must use the dedicated protocol. */
internal fun permissionSettingsCommands(layer: String, before: JSONObject, after: JSONObject): List<ClientCommand> {
    val destination = WritableScopeDto.valueOf(layer.uppercase())
    fun strings(value: JSONObject, key: String): List<String> {
        val array = value.optJSONArray(key) ?: return emptyList()
        return (0 until array.length()).map { array.getString(it) }.distinct()
    }
    return buildList {
        listOf("allow", "deny", "ask").forEach { behavior ->
            val old = strings(before,behavior)
            val next = strings(after,behavior)
            val addedRules = next.filterNot(old::contains)
            val removedRules = old.filterNot(next::contains)
            if (addedRules.isNotEmpty() || removedRules.isNotEmpty()) add(ClientCommand.UpdatePermissionRules(destination,PermissionBehaviorDto.valueOf(behavior.uppercase()),addedRules,removedRules))
        }
        if (after.has("defaultMode") && before.optString("defaultMode") != after.optString("defaultMode")) {
            val mode = after.getString("defaultMode")
            require(mode != "bypassPermissions") { "Bypass cannot be persisted as a default permission mode. Use the session permission control." }
            add(ClientCommand.SetDefaultPermissionMode(destination,mode))
        }
        val oldDirs = strings(before,"additionalDirectories")
        val newDirs = strings(after,"additionalDirectories")
        val addDirs = newDirs.filterNot(oldDirs::contains)
        val removeDirs = oldDirs.filterNot(newDirs::contains)
        if (addDirs.isNotEmpty() || removeDirs.isNotEmpty()) add(ClientCommand.UpdateWorkspaceDirectories(destination,addDirs,removeDirs))
    }
}

internal fun permissionSnapshotMatches(expected: JSONObject, actual: JSONObject): Boolean {
    fun values(obj: JSONObject, key: String): Set<String> {
        val array = obj.optJSONArray(key) ?: return emptySet()
        return (0 until array.length()).map { array.optString(it) }.toSet()
    }
    return listOf("allow", "deny", "ask", "additionalDirectories").all { values(expected,it) == values(actual,it) } &&
        (!expected.has("defaultMode") || expected.optString("defaultMode") == actual.optString("defaultMode"))
}
