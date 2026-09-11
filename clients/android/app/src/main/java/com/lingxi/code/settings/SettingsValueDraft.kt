package com.lingxi.code.settings

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject

/** A refresh may update pristine drafts, but never overwrites an unsaved user edit. */
internal class SettingsValueDraft(initial: String) {
    var value by mutableStateOf(initial)
        private set
    var baseline by mutableStateOf(initial)
        private set
    var externalChange by mutableStateOf(false)
        private set
    val dirty: Boolean get() = !sameJson(value, baseline)
    fun edit(next: String) { value = next }
    fun observe(next: String) {
        if (!dirty || sameJson(value,next)) {
            value = next
            baseline = next
            externalChange = false
        } else externalChange = !sameJson(next,baseline)
    }
    fun reset(next: String) { value = next; baseline = next; externalChange = false }
}
private fun sameJson(a: String, b: String): Boolean = runCatching {
    jsonEquivalent(JSONObject("{\"value\":$a}").opt("value"),JSONObject("{\"value\":$b}").opt("value"))
}.getOrDefault(a == b)
