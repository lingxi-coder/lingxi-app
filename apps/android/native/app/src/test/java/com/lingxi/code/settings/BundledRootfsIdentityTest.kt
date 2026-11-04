package com.lingxi.code.settings

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class BundledRootfsIdentityTest {
    private val sha = "a".repeat(64)
    private val brand = RootfsBrandIdentity("lingxi", "lingxi", ".lingxi")
    private fun pins() = """{"schema_version":2,"source_toolchain_pins_sha256":"${"b".repeat(64)}","rootfs":{"version":"fixture-v2","release_archives":{"arm64-v8a":{"filename":"rootfs.tar.gz","sha256":"$sha","size_bytes":123}}}}"""
    private fun manifest() = """{"platform":"android","runtime":"android-proot","abi":"arm64","brand_profile":{"brand_id":"lingxi","linux_user":"lingxi","config_dir_name":".lingxi"},"rootfs_version":"fixture-v2","archive":{"filename":"rootfs.tar.gz","sha256":"$sha","size_bytes":123}}"""

    @Test fun accepts_only_matching_packaged_release_identity() {
        val identity = bundledRootfsIdentity(pins(), manifest(), "arm64-v8a", brand)
        assertEquals("fixture-v2", identity.version)
        assertEquals(sha, identity.sha256)
        assertEquals(123L, identity.sizeBytes)
    }

    @Test fun rejects_source_archive_or_missing_abi_instead_of_falling_back() {
        val sourceOnly = JSONObject(pins())
        val rootfs = sourceOnly.getJSONObject("rootfs")
        rootfs.put("archives", rootfs.remove("release_archives"))
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(sourceOnly.toString(), manifest(), "arm64-v8a", brand) }
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), manifest(), "x86_64", brand) }
    }

    @Test fun rejects_stale_manifest_archive_or_version() {
        val staleVersion = JSONObject(manifest()).put("rootfs_version", "old-version")
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), staleVersion.toString(), "arm64-v8a", brand) }
        val staleArchive = JSONObject(manifest())
        staleArchive.getJSONObject("archive").put("sha256", "c".repeat(64))
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), staleArchive.toString(), "arm64-v8a", brand) }
    }

    @Test fun rejects_missing_or_mismatched_brand_profile() {
        val missing = JSONObject(manifest()).apply { remove("brand_profile") }
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), missing.toString(), "arm64-v8a", brand) }
        for (field in listOf("brand_id", "linux_user", "config_dir_name")) {
            val wrong = JSONObject(manifest())
            wrong.getJSONObject("brand_profile").put(field, "other")
            assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), wrong.toString(), "arm64-v8a", brand) }
            val wrongType = JSONObject(manifest())
            wrongType.getJSONObject("brand_profile").put(field, 123)
            assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), wrongType.toString(), "arm64-v8a", brand) }
        }
        val extra = JSONObject(manifest())
        extra.getJSONObject("brand_profile").put("unexpected", true)
        assertThrows(IllegalStateException::class.java) { bundledRootfsIdentity(pins(), extra.toString(), "arm64-v8a", brand) }
    }
}
