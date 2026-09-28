package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class BundledRootfsIdentityTest {
    private val sha = "a".repeat(64)
    private fun pins() = """{"schema_version":2,"source_toolchain_pins_sha256":"${"b".repeat(64)}","rootfs":{"version":"fixture-v2","release_archives":{"arm64-v8a":{"filename":"rootfs.tar.gz","sha256":"$sha","size_bytes":123}}}}"""
    private fun manifest() = """{"platform":"android","runtime":"android-proot","abi":"arm64","rootfs_version":"fixture-v2","archive":{"filename":"rootfs.tar.gz","sha256":"$sha","size_bytes":123}}"""

    @Test fun accepts_only_matching_packaged_release_identity() {
        val identity = bundledRootfsIdentity(pins(), manifest(), "arm64-v8a")
        assertEquals("fixture-v2", identity.version)
        assertEquals(sha, identity.sha256)
        assertEquals(123L, identity.sizeBytes)
    }

    @Test fun rejects_source_archive_or_missing_abi_instead_of_falling_back() {
        val sourceOnly = JSONObject(pins())
        val rootfs = sourceOnly.getJSONObject("rootfs")
        rootfs.put("archives", rootfs.remove("release_archives"))
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(sourceOnly.toString(), manifest(), "arm64-v8a") }
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), manifest(), "x86_64") }
    }

    @Test fun rejects_stale_manifest_archive_or_version() {
        val staleVersion = JSONObject(manifest()).put("rootfs_version", "old-version")
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), staleVersion.toString(), "arm64-v8a") }
        val staleArchive = JSONObject(manifest())
        staleArchive.getJSONObject("archive").put("sha256", "c".repeat(64))
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), staleArchive.toString(), "arm64-v8a") }
    }
}
