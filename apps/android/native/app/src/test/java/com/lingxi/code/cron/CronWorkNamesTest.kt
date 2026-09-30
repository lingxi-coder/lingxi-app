package com.lingxi.code.cron

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class CronWorkNamesTest {
    @Test
    fun sameExactAlarmInstantMapsToSameUniqueDispatch() {
        assertEquals(
            CronWorkNames.dispatch(1234L),
            CronWorkNames.dispatch(1234L),
        )
        assertNotEquals(
            CronWorkNames.dispatch(1234L),
            CronWorkNames.dispatch(1235L),
        )
    }

    @Test
    fun occurrenceIncludesScopeTaskAndScheduleAnchor() {
        assertEquals(
            "cron-occurrence-project-a-task-b-1234",
            CronWorkNames.occurrence("project-a", "task-b", 1234L),
        )
    }
}
