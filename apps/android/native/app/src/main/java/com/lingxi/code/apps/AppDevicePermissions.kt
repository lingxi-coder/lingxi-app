package com.lingxi.code.apps

import android.content.Context

/** Native feature grants never imply Linux process isolation or execution consent. */
class AppDevicePermissions(context: Context) {
    private val store = context.getSharedPreferences("local-app-v2-device-grants", Context.MODE_PRIVATE)
    fun allowed(appId: String, digest: String, capability: String): Boolean =
        store.getStringSet("$appId:$digest", emptySet()).orEmpty().contains(capability)
    fun granted(appId: String, digest: String): Set<String> = store.getStringSet("$appId:$digest", emptySet()).orEmpty().toSet()
    fun grant(appId: String, digest: String, capabilities: Set<String>) {
        require(capabilities.all { it in supported }) { "Unsupported native capability" }
        store.edit().putStringSet("$appId:$digest", capabilities.toSet()).apply()
    }
    fun revoke(appId: String) {
        val edit = store.edit()
        store.all.keys.filter { it.startsWith("$appId:") }.forEach(edit::remove)
        edit.apply()
    }
    companion object {
        val supported = setOf("camera", "photos", "files", "microphone", "clipboard", "share", "notifications", "haptics")
    }
}
