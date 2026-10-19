package com.lingxi.code.navigation

import androidx.lifecycle.SavedStateHandle
import androidx.lifecycle.ViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

/** One persisted, bounded FIFO for cold starts, warm intents and configuration changes. */
class DeepLinkInbox(private val saved: SavedStateHandle) : ViewModel() {
    private val mutable = MutableStateFlow(saved.get<ArrayList<String>>("deepLinks")?.filter { DeepLink.parse(it) != null }.orEmpty().take(CAPACITY))
    val links = mutable.asStateFlow()
    fun deliver(raw: String?, restoringActivity: Boolean = false): Boolean {
        if (raw != null && restoringActivity && saved.get<String>("lastLinkIntent") == raw) return true
        val accepted = enqueue(raw)
        if (accepted) saved["lastLinkIntent"] = raw
        return accepted
    }
    fun enqueue(raw: String?): Boolean {
        if (DeepLink.parse(raw) == null) return false
        if (raw in mutable.value) return true
        if (mutable.value.size >= CAPACITY) return false
        update(mutable.value + raw!!)
        return true
    }
    fun next(readiness: DeepLinkReadiness): String? = mutable.value.firstOrNull().takeIf { readiness.canNavigate }
    fun complete(raw: String) {
        if (mutable.value.firstOrNull() == raw) update(mutable.value.drop(1))
    }
    private fun update(value: List<String>) {
        saved["deepLinks"] = ArrayList(value)
        mutable.value = value
    }
    companion object { const val CAPACITY = 16 }
}

data class DeepLinkReadiness(
    val foreground: Boolean,
    val onboardingComplete: Boolean,
    val sourceBound: Boolean,
    val engineBusy: Boolean,
    val blockingSurfaceOpen: Boolean,
) {
    val canNavigate: Boolean get() = foreground && onboardingComplete && sourceBound && !engineBusy && !blockingSurfaceOpen
}
