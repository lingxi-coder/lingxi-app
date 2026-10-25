package com.lingxi.code.apps

import org.junit.Assert.assertEquals
import org.junit.Test

class AppsModelsTest {
    @Test
    fun recentAppsAreTheLastThreeOpened() {
        val apps = listOf(
            MyApp("a", "A", "", 1),
            MyApp("b", "B", "", null),
            MyApp("c", "C", "", 3),
            MyApp("d", "D", "", 4),
            MyApp("e", "E", "", 2),
        )
        assertEquals(listOf("d", "c", "e"), recentApps(apps).map(MyApp::appId))
    }

    @Test
    fun matchingAppsSearchesNamesIgnoringCase() {
        val apps = listOf(MyApp("a", "Water Tracker", "", null), MyApp("b", "记账本", "", null))
        assertEquals(emptyList<MyApp>(), matchingApps(apps, "  "))
        assertEquals(listOf("a"), matchingApps(apps, "water").map(MyApp::appId))
        assertEquals(listOf("b"), matchingApps(apps, "账").map(MyApp::appId))
    }

    @Test
    fun appInitialIsTheFirstCharacter() {
        assertEquals("记", appInitial("记账本"))
        assertEquals("C", appInitial("  calculator"))
        assertEquals("😀", appInitial("😀 Fun"))
        assertEquals("?", appInitial(" "))
    }

}
