package com.lingxi.code.drawer

import org.junit.Assert.assertNull
import org.junit.Test

class DrawerProductionDataTest {
    @Test
    fun defaultDataSource_isUnavailableAndContainsNoPrototypeRows() {
        val data = DrawerProductionData()

        assertNull(data.workspaces)
        assertNull(data.projects)
        assertNull(data.crons)
    }
}
