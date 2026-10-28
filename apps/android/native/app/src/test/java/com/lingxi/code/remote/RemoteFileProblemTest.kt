package com.lingxi.code.remote

import com.lingxi.code.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RemoteFileProblemTest {
    @Test fun nativeTimeoutHasAnActionableLocalizedMessage() {
        assertEquals(R.string.remote_file_timeout, remoteFileProblemResource("deadline has elapsed"))
    }

    @Test fun unrelatedErrorsKeepTheirOriginalDetails() {
        assertNull(remoteFileProblemResource("file checksum mismatch"))
    }

    @Test fun downloadFailureExplainsThePossibleIncompleteDestination() {
        val message = remoteFileDownloadProblem("deadline has elapsed", "Destination may be incomplete") {
            assertEquals(R.string.remote_file_timeout, it)
            "Connection timed out"
        }
        assertEquals("Connection timed out\nDestination may be incomplete", message)
        assertEquals("Destination may be incomplete", remoteFileDownloadProblem(null, "Destination may be incomplete") { "unused" })
    }
}
