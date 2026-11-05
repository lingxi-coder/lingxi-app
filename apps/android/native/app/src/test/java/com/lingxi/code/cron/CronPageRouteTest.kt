package com.lingxi.code.cron

import androidx.compose.runtime.saveable.SaverScope
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The scheduled-task page's route (decision D14: a pushed page of the Chats tab): fallbacks and saved state. */
class CronPageRouteTest {
    private fun save(route: CronPageRoute?): Any? = with(CronPageRoute.Saver) { SaverScope { true }.save(route) }

    private fun roundTrip(route: CronPageRoute?): CronPageRoute? = CronPageRoute.Saver.restore(save(route)!!)

    @Test
    fun aBlankTaskKeyOpensTheTaskList() {
        assertEquals(CronPageRoute.Tasks, CronPageRoute.task(null))
        assertEquals(CronPageRoute.Tasks, CronPageRoute.task(""))
        assertEquals(CronPageRoute.Tasks, CronPageRoute.task("   "))
        assertEquals(CronPageRoute.Task("scope:1"), CronPageRoute.task("scope:1"))
    }

    @Test
    fun aBlankPromptOpensTheTaskListInsteadOfAnEmptyForm() {
        assertEquals(CronPageRoute.Tasks, CronPageRoute.new(""))
        assertEquals(CronPageRoute.Tasks, CronPageRoute.new("  \n"))
        assertEquals(CronPageRoute.New("water the plants"), CronPageRoute.new("water the plants"))
    }

    @Test
    fun everyRouteSurvivesRotationAndProcessDeath() {
        listOf(
            CronPageRoute.Tasks,
            CronPageRoute.Task("global:abc"),
            CronPageRoute.Run("run-42"),
            CronPageRoute.New("every morning, summarise my mail"),
        ).forEach { route -> assertEquals(route, roundTrip(route)) }
    }

    @Test
    fun noPageAtAllIsSavedAsNone() {
        assertNull(roundTrip(null))
    }

    @Test
    fun savedStateThatNamesNoPageOrNoArgumentRestoresToNone() {
        assertNull(CronPageRoute.Saver.restore(listOf("task")))
        assertNull(CronPageRoute.Saver.restore(listOf("run")))
        assertNull(CronPageRoute.Saver.restore(listOf("renamed-page", "x")))
        assertNull(CronPageRoute.Saver.restore(emptyList<Any>()))
    }
}
