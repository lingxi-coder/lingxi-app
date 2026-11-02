package com.lingxi.code.project

import com.lingxi.code.model.SessionMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ProjectStoreStateTest {
    @Test
    fun unreadableIndexesBlockOwnerSelectionUntilAValidatedReload() {
        val conflict = ProjectSyncConflict("p", "file", "local", "external", null)
        val previous = ProjectStoreState(loading = true, activeProjectId = "retained", conflicts = listOf(conflict))
        val failed = repositoryFailureState(previous, null, "conflicting owner", listOf("project.p"))
        assertFalse(failed.loading)
        assertTrue(failed.storageUnavailable)
        assertEquals("retained", failed.activeProjectId)
        assertEquals("conflicting owner", failed.errorMessage)
        assertEquals(listOf("project.p"), failed.unavailableScopes)

        val recovered = ProjectStoreState(loading = false, activeProjectId = "valid")
        val operationFailure = repositoryFailureState(failed, recovered, "operation failed")
        assertFalse(operationFailure.storageUnavailable)
        assertEquals("valid", operationFailure.activeProjectId)
        assertEquals(listOf(conflict), operationFailure.conflicts)
        assertEquals("operation failed", operationFailure.errorMessage)
    }

    @Test
    fun recordedOwnerMustAgreeWithIndexedWorkspaceAndMode() {
        val id = "19587a33-0725-48db-abca-8a2aed345f6b"
        val chat = ScopeStateStore.SessionOwner("global", SessionMode.Chat)
        requireMatchingSessionOwners(mapOf(id to chat), mapOf(id to chat))
        requireMatchingSessionOwners(mapOf(id to chat), emptyMap())
        val conflict = runCatching {
            requireMatchingSessionOwners(mapOf(id to chat),
                mapOf(id to ScopeStateStore.SessionOwner("project.p", SessionMode.Code)))
        }.exceptionOrNull() as ProjectCatalogUnavailableException
        assertEquals(listOf("global", "project.p"), conflict.scopes)
    }

    @Test
    fun missingOwnerRegistryIsAllowedOnlyBeforeAnyOwnerOrPriorRegistry() {
        requireOwnerRegistryPresent(present = false, observedBefore = false, hasIndexedOwners = false)
        requireOwnerRegistryPresent(present = true, observedBefore = true, hasIndexedOwners = true)
        for ((observed, indexed) in listOf(true to false, false to true)) {
            val failure = runCatching { requireOwnerRegistryPresent(false, observed, indexed) }
                .exceptionOrNull() as ProjectCatalogUnavailableException
            assertEquals(listOf("owner-registry"), failure.scopes)
        }
    }
}
