package com.lingxi.code.conversation

import androidx.test.ext.junit.runners.AndroidJUnit4
import com.lingxi.code.bindings.client.slashCommandArgumentCount
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class NativeBindingUiTest {
    @Test fun nativeBindingsInitializeWithCurrentArgumentParser() {
        com.lingxi.code.bindings.android.uniffiEnsureInitialized()
        com.lingxi.code.bindings.runtime.uniffiEnsureInitialized()
        com.lingxi.code.bindings.client.uniffiEnsureInitialized()
        assertEquals(2u, slashCommandArgumentCount("one 'two three' "))
        assertEquals(0u, slashCommandArgumentCount(""))
    }
}
