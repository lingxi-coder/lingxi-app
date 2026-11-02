package com.lingxi.code

import com.lingxi.code.conversation.ConversationSource
import com.lingxi.code.settings.ProviderAccountsRepository
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ProviderAccountsReadOnlyTest {
    @Test fun repeatedSnapshotsNeverDispatchAnAccountMutation() = runBlocking {
        var reads = 0
        val source = object : ConversationSource {
            override suspend fun providerAccountsSnapshot(): String {
                reads += 1
                return """{"storeScope":"test-scope","configRevision":0,"accounts":[],"health":[],"providerPolicies":{}}"""
            }
            override suspend fun providerAccountCommand(requestJson: String, credential: String?): String =
                error("Reading account state must not write credentials or dispatch a provider test")
        }
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repeat(3) { repository.refresh() }
        assertEquals(3, reads)
        assertTrue(repository.state.value.loaded)
        assertTrue(repository.state.value.accounts.isEmpty())
    }
}
