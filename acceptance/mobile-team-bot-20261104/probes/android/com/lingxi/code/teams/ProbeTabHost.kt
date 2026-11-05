package com.lingxi.code.teams
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import com.lingxi.code.tabs.*
private object ProbeHost : TabHost {
    override fun select(tab: AppTab) {}
    override fun openSettings(route: String, origin: AppTab?) {}
    override fun reportDepth(tab: AppTab, depth: Int) {}
}
@Composable fun ProbeTabHost(content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalTabHost provides ProbeHost, content = content)
}
