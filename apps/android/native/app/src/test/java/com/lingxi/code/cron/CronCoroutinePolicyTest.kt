package com.lingxi.code.cron

import java.util.concurrent.CancellationException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class CronCoroutinePolicyTest {
    @Test
    fun cancellationIsNeverConvertedIntoRepositoryErrorState() {
        val cancellation = CancellationException("The coroutine scope left the composition")

        val thrown = assertThrows(CancellationException::class.java) {
            runCronCatching<Unit> { throw cancellation }
        }

        assertEquals(cancellation, thrown)
    }

    @Test
    fun ordinaryFailureRemainsAvailableForUiErrorState() {
        val result = runCronCatching<Unit> { error("broken cron store") }

        assertEquals("broken cron store", result.exceptionOrNull()?.message)
    }
}
