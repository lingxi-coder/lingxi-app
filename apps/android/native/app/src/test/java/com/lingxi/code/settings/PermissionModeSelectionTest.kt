package com.lingxi.code.settings

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.resetMain
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.test.setMain
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class PermissionModeSelectionTest {
    @Test
    fun selectionChangesOnlyAfterEngineSuccessAndSnapshotsDoNotReplaceIt() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val store = SettingsStore()
            val applied = CompletableDeferred<Unit>()
            store.setPermissionMode("acceptEdits") { applied.await() }
            runCurrent()
            assertEquals("auto", store.state.value.permissionMode)
            applied.complete(Unit)
            runCurrent()
            assertEquals("acceptEdits", store.state.value.permissionMode)
            assertNull(store.state.value.permissionModeError)
            store.setEffectivePermissionMode("plan")
            assertEquals("acceptEdits", store.state.value.permissionMode)
            assertEquals("plan", store.state.value.effectivePermissionMode)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun rejectedSelectionPreservesTheLastSuccessfulMode() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val store = SettingsStore()
            store.setPermissionMode("plan") {}
            runCurrent()
            store.setPermissionMode("bypassPermissions") { error("bypass is unavailable") }
            runCurrent()
            assertEquals("plan", store.state.value.permissionMode)
            assertEquals("bypass is unavailable", store.state.value.permissionModeError)
        } finally {
            Dispatchers.resetMain()
        }
    }

    @Test
    fun rapidChoicesApplyInOrderAndFailureCannotRollbackAnEarlierSuccess() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val store = SettingsStore()
            val first = CompletableDeferred<Unit>()
            val calls = mutableListOf<String>()
            store.setPermissionMode("plan") { calls += it; first.await() }
            store.setPermissionMode("dontAsk") { calls += it; error("rejected") }
            runCurrent()
            assertEquals(listOf("plan"), calls)
            first.complete(Unit)
            runCurrent()
            assertEquals(listOf("plan", "dontAsk"), calls)
            assertEquals("plan", store.state.value.permissionMode)
        } finally {
            Dispatchers.resetMain()
        }
    }
}
