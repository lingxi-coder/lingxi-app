package com.lingxi.code.cron

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CronSchedulePolicyTest {
    @Test
    fun repeatingSchedulesRespectAndroidFifteenMinuteFloor() {
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("* * * * *", recurring = true))
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("*/5 * * * *", recurring = true))
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("0,10 * * * *", recurring = true))
        assertTrue(CronSchedulePolicy.isAndroidSupportedRecurring("*/15 * * * *", recurring = true))
        assertTrue(CronSchedulePolicy.isAndroidSupportedRecurring("0,15,30,45 * * * *", recurring = true))
        assertTrue(CronSchedulePolicy.isAndroidSupportedRecurring("5 * * * *", recurring = true))
    }

    @Test
    fun oneShotIsExemptFromRecurringFloor() {
        assertTrue(CronSchedulePolicy.isAndroidSupportedRecurring("* * * * *", recurring = false))
    }

    @Test
    fun malformedScheduleFailsClosedForRecurringTask() {
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("not cron", recurring = true))
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("61 * * * *", recurring = true))
        assertFalse(CronSchedulePolicy.isAndroidSupportedRecurring("*/0 * * * *", recurring = true))
    }
}
