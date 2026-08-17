package com.lingxi.code.localapps

import java.io.File
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger
import org.junit.After
import org.junit.AfterClass
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.json.JSONArray
import org.json.JSONObject

class LocalAppRuntimeAssetsTest {
    @Before
    fun reset() {
        LocalAppRuntimeAssets.resetForTests()
    }

    @After
    fun resetAgain() {
        LocalAppRuntimeAssets.resetForTests()
    }

    @Test
    fun `a cold staging run returns the promised root without blocking on extraction`() {
        val started = CountDownLatch(1)
        val runs = AtomicInteger()
        val stage = {
            runs.incrementAndGet()
            started.countDown()
            Thread.sleep(STAGE_MS)
            STAGED_ROOT
        }

        val begin = System.nanoTime()
        val first = LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, STAGED_ROOT, stage)
        val elapsedMs = (System.nanoTime() - begin) / 1_000_000

        assertEquals(STAGED_ROOT, first)
        assertTrue("caller was parked for ${elapsedMs}ms", elapsedMs < STAGE_MS / 2)
        assertTrue("staging must actually have started", started.await(5, TimeUnit.SECONDS))
        assertEquals(LocalAppRuntimeStaging.Staging, LocalAppRuntimeAssets.stagingStatus())

        val second =
            LocalAppRuntimeAssets.prepareWithin(10_000, STAGED_ROOT) { error("must not re-stage") }
        assertEquals(STAGED_ROOT, second)
        assertEquals(1, runs.get())
        assertEquals(LocalAppRuntimeStaging.Ready, awaitStatus { it == LocalAppRuntimeStaging.Ready })
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(0, STAGED_ROOT) { error("must not re-stage") },
        )
    }

    @Test
    fun `a promised root that later fails reads as unavailable and still retries`() {
        val release = CountDownLatch(1)
        val runs = AtomicInteger()

        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, STAGED_ROOT) {
                runs.incrementAndGet()
                assertTrue(release.await(10, TimeUnit.SECONDS))
                null
            },
        )
        assertEquals(LocalAppRuntimeStaging.Staging, LocalAppRuntimeAssets.stagingStatus())

        release.countDown()
        assertEquals(
            LocalAppRuntimeStaging.Unavailable,
            awaitStatus { it == LocalAppRuntimeStaging.Unavailable },
        )

        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000, STAGED_ROOT) {
                runs.incrementAndGet()
                STAGED_ROOT
            },
        )
        assertEquals(2, runs.get())
        assertEquals(LocalAppRuntimeStaging.Ready, LocalAppRuntimeAssets.stagingStatus())
    }

    @Test
    fun `a run that produces no runtime without a promised root reads as unavailable and retries`() {
        val runs = AtomicInteger()

        assertNull(LocalAppRuntimeAssets.prepareWithin(10_000) { runs.incrementAndGet(); null })
        assertEquals(LocalAppRuntimeStaging.Unavailable, LocalAppRuntimeAssets.stagingStatus())

        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000, STAGED_ROOT) {
                runs.incrementAndGet()
                STAGED_ROOT
            },
        )
        assertEquals(2, runs.get())
        assertEquals(LocalAppRuntimeStaging.Ready, LocalAppRuntimeAssets.stagingStatus())
    }

    @Test
    fun `a runtime staged before the first caller reads as ready and adds no notice`() {
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000, STAGED_ROOT) { STAGED_ROOT },
        )

        assertEquals(LocalAppRuntimeStaging.Ready, LocalAppRuntimeAssets.stagingStatus())
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeAssets.stagingStatus()))
        assertEquals(ENGINE_DETAIL, LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true))
        assertNull(LocalAppRuntimeAssets.generationDetail(null, failed = true))
    }

    @Test
    fun `a memoized runtime does not reload the asset manifest plan`() {
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000, STAGED_ROOT) { STAGED_ROOT },
        )
        var planReads = 0

        val result =
            LocalAppRuntimeAssets.prepareWithinPlan(
                budgetMs = 10_000,
                planProvider = {
                    planReads += 1
                    error("the manifest plan must not be reloaded after staging succeeds")
                },
                stage = { error("the runtime must not be staged twice") },
            )

        assertEquals(STAGED_ROOT, result)
        assertEquals(0, planReads)
    }

    @Test
    fun `idle and ready stay silent while no-ready-runtime states explain themselves`() {
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Idle))
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Ready))

        val notices = listOf(
            LocalAppRuntimeStaging.Staging,
            LocalAppRuntimeStaging.Unavailable,
        ).map { LocalAppRuntimeAssets.noticeFor(it) }

        notices.forEach { assertNotNull(it) }
        assertTrue(notices.all { !it.isNullOrBlank() })
        assertEquals(2, notices.toSet().size)
    }

    @Test
    fun `only a failed generation job carries the runtime explanation`() {
        val release = CountDownLatch(1)
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, STAGED_ROOT) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        val notice = LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Staging)!!

        assertEquals(ENGINE_DETAIL, LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = false))

        val annotated = LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true)
        assertNotNull(annotated)
        assertTrue(annotated!!.startsWith(ENGINE_DETAIL))
        assertTrue(annotated.contains(notice))
        assertEquals(notice, LocalAppRuntimeAssets.generationDetail(null, failed = true))

        release.countDown()
    }

    @Test
    fun `a process with no ready root annotates failures even when they never name the runtime`() {
        assertNull(LocalAppRuntimeAssets.prepareWithin(10_000) { null })
        assertEquals(LocalAppRuntimeStaging.Unavailable, LocalAppRuntimeAssets.stagingStatus())
        assertTrue(
            LocalAppRuntimeAssets.generationDetail(LLM_DETAIL, failed = true)!!
                .contains(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Unavailable)!!),
        )

        val release = CountDownLatch(1)
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, STAGED_ROOT) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        assertEquals(LocalAppRuntimeStaging.Staging, LocalAppRuntimeAssets.stagingStatus())
        assertTrue(
            LocalAppRuntimeAssets.generationDetail(LLM_DETAIL, failed = true)!!
                .contains(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Staging)!!),
        )
        release.countDown()
    }

    @Test
    fun `a matching manifest without a regular vite file is not ready`() {
        val parent = Files.createTempDirectory("local-app-runtime-ready").toFile()
        val root =
            File(parent, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .apply { mkdirs() }
        root.deleteOnExit()
        File(root, "runtime-manifest.json").writeText("manifest")

        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(root, "manifest"))

        val viteDir = File(root, "node_modules/vite/bin").apply { mkdirs() }
        File(viteDir, "vite.js").mkdir()
        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(root, "manifest"))

        File(viteDir, "vite.js").deleteRecursively()
        File(viteDir, "vite.js").writeText("console.log('vite')")
        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(root, "manifest"))
        assertTrue(LocalAppRuntimeAssets.publishReadyMarker(root))
        assertTrue(LocalAppRuntimeAssets.runtimeIsReady(root, "manifest"))

        parent.deleteRecursively()
    }

    @Test
    fun `a throwing plan provider fails soft instead of crashing prepare`() {
        var sawNullPlan = false

        val result = LocalAppRuntimeAssets.prepareWithinPlan(
            budgetMs = 10_000,
            planProvider = { error("boom") },
            stage = { plan ->
                sawNullPlan = plan == null
                null
            },
        )

        assertNull(result)
        assertTrue(sawNullPlan)
        assertEquals(LocalAppRuntimeStaging.Unavailable, LocalAppRuntimeAssets.stagingStatus())
    }

    @Test
    fun `a failed promote leaves staging intact and never copies into destination`() {
        val parent = Files.createTempDirectory("local-app-runtime-promote").toFile()
        val destination = File(parent, "live").apply {
            mkdirs()
            File(this, "old.txt").writeText("old")
        }
        val staging = File(parent, "live.staging").apply {
            mkdirs()
            File(this, "new.txt").writeText("new")
        }

        val published =
            LocalAppRuntimeAssets.replaceAtomically(staging, destination) { _, _ -> false }

        assertFalse(published)
        assertTrue(staging.exists())
        assertTrue(File(staging, "new.txt").isFile)
        assertFalse(destination.exists())

        parent.deleteRecursively()
    }

    @Test
    fun `failure marker writes a bounded reason and clears on recovery`() {
        val parent = Files.createTempDirectory("local-app-runtime-failure-marker").toFile()
        val destination =
            File(parent, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        val longReason = buildString { repeat(600) { append('x') } }

        assertTrue(LocalAppRuntimeAssets.publishReadyMarker(destination))
        LocalAppRuntimeAssets.publishFailureMarker(destination, longReason)
        val marker = LocalAppRuntimeAssets.failureMarkerFor(destination)

        assertTrue(marker.isFile)
        assertTrue(marker.readText().length <= 512)
        assertFalse(LocalAppRuntimeAssets.readyMarkerFor(destination).exists())

        LocalAppRuntimeAssets.deleteFailureMarker(destination)
        assertFalse(marker.exists())

        parent.deleteRecursively()
    }

    @Test
    fun `inventory validation enforces schema platform and file hashes`() {
        val root = Files.createTempDirectory("local-app-runtime-inventory").toFile()
        writeInventoryFixture(root)

        val manifest = runtimeManifestJson(root, platform = "android")
        assertTrue(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))

        File(root, "runtime-policy.json").writeText("tampered")
        assertFalse(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))
        assertFalse(
            LocalAppRuntimeAssets.runtimeInventoryIsValid(
                root,
                runtimeManifestJson(root, platform = "ios"),
            ),
        )

        root.deleteRecursively()
    }

    @Test
    fun `inventory validation rejects manifests that omit required runtime files`() {
        val root = Files.createTempDirectory("local-app-runtime-required").toFile()
        writeInventoryFixture(root)

        val manifest =
            runtimeManifestJson(
                root = root,
                platform = "android",
                dropPaths = setOf("runtime.spdx.json"),
            )

        assertFalse(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))
        root.deleteRecursively()
    }

    @Test
    fun `inventory validation rejects manifests with invalid path components`() {
        val root = Files.createTempDirectory("local-app-runtime-invalid-paths").toFile()
        writeInventoryFixture(root)

        val manifest =
            runtimeManifestJson(
                root = root,
                platform = "android",
                pathOverrides =
                    mapOf(
                        "runtime-policy.json" to "dir/../runtime-policy.json",
                    ),
            )

        assertFalse(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))
        root.deleteRecursively()
    }

    @Test
    fun `inventory validation rejects extra files outside the manifest closure`() {
        val root = Files.createTempDirectory("local-app-runtime-extra-file").toFile()
        writeInventoryFixture(root)
        File(root, "extra.txt").writeText("unexpected")

        val manifest = runtimeManifestJson(root, platform = "android", dropPaths = setOf("extra.txt"))

        assertFalse(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))
        root.deleteRecursively()
    }

    @Test
    fun `inventory validation rejects file entries backed by symlinks and prepare fails soft`() {
        val root = Files.createTempDirectory("local-app-runtime-file-symlink").toFile()
        writeInventoryFixture(root)
        val externalTarget = Files.createTempFile("local-app-runtime-target", ".json").toFile()
        externalTarget.writeText(File(root, "runtime-policy.json").readText())
        File(root, "runtime-policy.json").delete()
        Files.createSymbolicLink(
            File(root, "runtime-policy.json").toPath(),
            externalTarget.toPath(),
        )
        val manifest = runtimeManifestJson(root, platform = "android")

        assertFalse(LocalAppRuntimeAssets.runtimeInventoryIsValid(root, manifest))
        assertEquals(
            root.absolutePath,
            LocalAppRuntimeAssets.prepareWithinPlan(
                budgetMs = 10_000,
                planProvider = {
                    LocalAppRuntimeAssets.RuntimePlan(
                        expectedManifest = manifest,
                        destination = root,
                    )
                },
                stage = { plan ->
                    if (
                        plan != null &&
                        LocalAppRuntimeAssets.runtimeInventoryIsValid(
                            root = plan.destination,
                            manifestJson = plan.expectedManifest,
                        )
                    ) {
                        plan.destination.absolutePath
                    } else {
                        null
                    }
                },
            ),
        )
        assertEquals(
            LocalAppRuntimeStaging.Unavailable,
            awaitStatus { it == LocalAppRuntimeStaging.Unavailable },
        )

        root.deleteRecursively()
        externalTarget.delete()
    }

    @Test
    fun `manifest changes produce different promised destinations`() {
        val container = File("/tmp/local-app-runtime")

        val first = LocalAppRuntimeAssets.destinationForManifest(container, "manifest-a")
        val second = LocalAppRuntimeAssets.destinationForManifest(container, "manifest-b")

        assertTrue(first.parentFile == container)
        assertTrue(second.parentFile == container)
        assertFalse(first.absolutePath == second.absolutePath)
    }

    @Test
    fun `sealed runtime files become read-only while the tree stays removable`() {
        val root = Files.createTempDirectory("local-app-runtime-seal").toFile()
        val nestedDir = File(root, "node_modules/vite/bin").apply { mkdirs() }
        val file = File(nestedDir, "vite.js").apply { writeText("console.log('vite')") }

        LocalAppRuntimeAssets.sealRegularFilesReadOnly(root)

        assertFalse(Files.isWritable(file.toPath()))
        assertTrue(nestedDir.list() != null)
        assertTrue(root.deleteRecursively())
    }

    @Test
    fun `ready marker lifecycle gates readiness and cleanup removes obsolete markers`() {
        val parent = Files.createTempDirectory("local-app-runtime-ready-marker").toFile()
        val keep =
            File(parent, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        val obsolete =
            File(parent, "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")
        File(keep, "node_modules/vite/bin").mkdirs()
        File(keep, "node_modules/vite/bin/vite.js").writeText("vite")
        File(keep, "runtime-manifest.json").writeText("manifest")
        obsolete.mkdirs()
        LocalAppRuntimeAssets.publishReadyMarker(keep)
        LocalAppRuntimeAssets.publishReadyMarker(obsolete)
        LocalAppRuntimeAssets.publishFailureMarker(obsolete, "stale")

        assertTrue(LocalAppRuntimeAssets.runtimeIsReady(keep, "manifest"))

        LocalAppRuntimeAssets.cleanupObsoleteDigests(parent, keep.name)

        assertTrue(LocalAppRuntimeAssets.readyMarkerFor(keep).isFile)
        assertFalse(obsolete.exists())
        assertFalse(LocalAppRuntimeAssets.readyMarkerFor(obsolete).exists())
        assertFalse(LocalAppRuntimeAssets.failureMarkerFor(obsolete).exists())

        LocalAppRuntimeAssets.deleteReadyMarker(keep)
        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(keep, "manifest"))

        parent.deleteRecursively()
    }

    @Test
    fun `corrupt ready markers do not satisfy readiness`() {
        val parent = Files.createTempDirectory("local-app-runtime-corrupt-ready").toFile()
        val destination =
            File(parent, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        File(destination, "node_modules/vite/bin").mkdirs()
        File(destination, "node_modules/vite/bin/vite.js").writeText("vite")
        File(destination, "runtime-manifest.json").writeText("manifest")

        assertTrue(LocalAppRuntimeAssets.publishReadyMarker(destination))
        assertTrue(LocalAppRuntimeAssets.runtimeIsReady(destination, "manifest"))

        LocalAppRuntimeAssets.readyMarkerFor(destination).writeText("wrong-digest")
        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(destination, "manifest"))
        assertTrue(LocalAppRuntimeAssets.publishReadyMarker(destination))
        assertTrue(LocalAppRuntimeAssets.runtimeIsReady(destination, "manifest"))

        LocalAppRuntimeAssets.readyMarkerFor(destination).writeText(destination.name + "\n")
        assertFalse(LocalAppRuntimeAssets.runtimeIsReady(destination, "manifest"))
        assertTrue(LocalAppRuntimeAssets.publishReadyMarker(destination))
        assertTrue(LocalAppRuntimeAssets.runtimeIsReady(destination, "manifest"))

        parent.deleteRecursively()
    }

    private fun awaitStatus(
        deadlineMs: Long = 10_000,
        predicate: (LocalAppRuntimeStaging) -> Boolean,
    ): LocalAppRuntimeStaging {
        val deadline = System.nanoTime() + deadlineMs * 1_000_000
        var status = LocalAppRuntimeAssets.stagingStatus()
        while (!predicate(status) && System.nanoTime() < deadline) {
            Thread.sleep(10)
            status = LocalAppRuntimeAssets.stagingStatus()
        }
        return status
    }

    private fun writeInventoryFixture(root: File) {
        File(root, "node_modules/vite/bin").mkdirs()
        File(root, "node_modules/@rolldown/binding-linux-arm64-musl").mkdirs()
        File(root, "node_modules/lightningcss-linux-arm64-musl").mkdirs()
        File(root, "node_modules/vite/bin/vite.js").writeText("vite")
        File(
            root,
            "node_modules/@rolldown/binding-linux-arm64-musl/rolldown-binding.linux-arm64-musl.node",
        ).writeText("rolldown")
        File(
            root,
            "node_modules/lightningcss-linux-arm64-musl/lightningcss.linux-arm64-musl.node",
        ).writeText("lightningcss")
        File(root, "runtime-policy.json").writeText("policy")
        File(root, "runtime-pins.json").writeText("pins")
        File(root, "runtime.spdx.json").writeText("sbom")
    }

    private fun runtimeManifestJson(
        root: File,
        platform: String,
        dropPaths: Set<String> = emptySet(),
        pathOverrides: Map<String, String> = emptyMap(),
    ): String {
        val files =
            root.walkTopDown()
                .filter(File::isFile)
                .filterNot { it.relativeTo(root).invariantSeparatorsPath == "runtime-manifest.json" }
                .map { file ->
                    val relative = file.relativeTo(root).invariantSeparatorsPath
                    JSONObject()
                        .put("path", pathOverrides[relative] ?: relative)
                        .put("kind", "file")
                        .put("size_bytes", file.length())
                        .put("sha256", sha256(file.readText()))
                }
                .filterNot { it.getString("path") in dropPaths }
                .toList()
                .sortedBy { it.getString("path") }

        val manifest =
            JSONObject()
            .put("schema_version", 1)
            .put("platform", platform)
            .put("read_only", true)
            .put("files", JSONArray(files))
            .toString()
        File(root, "runtime-manifest.json").writeText(manifest)
        return manifest
    }

    private fun sha256(text: String): String =
        MessageDigest.getInstance("SHA-256")
            .digest(text.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }

    companion object {
        const val BUDGET_MS = 150L
        const val STAGE_MS = 2_000L
        const val STAGED_ROOT =
            "/data/user/0/com.lingxi.code/files/local-app-runtime/" +
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        const val ENGINE_DETAIL =
            "not yet available: verified local-app Node runtime is unavailable; " +
                "stage local-app-runtime first"
        const val LLM_DETAIL = "生成失败：模型返回的方案无法解析，请重试。"

        @JvmStatic
        @AfterClass
        fun assertProcessSingletonLeftCold() {
            assertEquals(
                "this class must leave the singleton cold for the next class in this JVM",
                LocalAppRuntimeStaging.Idle,
                LocalAppRuntimeAssets.stagingStatus(),
            )
        }
    }
}
