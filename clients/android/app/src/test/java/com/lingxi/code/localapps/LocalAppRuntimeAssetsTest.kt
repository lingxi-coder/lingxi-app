package com.lingxi.code.localapps

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

class LocalAppRuntimeAssetsTest {
    /** The object is a process singleton; every test must see a cold process. */
    @Before
    fun reset() {
        LocalAppRuntimeAssets.resetForTests()
    }

    /**
     * …and so must every test class that follows. Gradle gives this module ONE
     * JVM — app/build.gradle.kts's `testOptions` sets neither `forkEvery` nor
     * `maxParallelForks` — and `LocalAppsViewModel.toUiGeneration` now reads
     * this singleton for every FAILED generation job, so a root left staged
     * here would silently append a runtime notice to another class's `detail`.
     * Resetting in @Before alone cannot prevent that; only this can.
     */
    @After
    fun resetAgain() {
        LocalAppRuntimeAssets.resetForTests()
    }

    /**
     * `prepare` is evaluated inside the `viewModel { initializer { … } }` block
     * that Compose runs on the MAIN thread, so a cold first launch must not
     * park it for the length of a ~200 MB asset extraction. The fake stage
     * returns on its own so a regression fails the assertions instead of
     * hanging the suite.
     */
    @Test
    fun `a cold staging run does not block the caller past its budget`() {
        val started = CountDownLatch(1)
        val runs = AtomicInteger()
        val stage = {
            runs.incrementAndGet()
            started.countDown()
            Thread.sleep(STAGE_MS)
            STAGED_ROOT
        }

        val begin = System.nanoTime()
        val first = LocalAppRuntimeAssets.prepareWithin(BUDGET_MS, stage)
        val elapsedMs = (System.nanoTime() - begin) / 1_000_000

        assertNull("a staging run that outlives the budget must not be awaited", first)
        assertTrue("caller was parked for ${elapsedMs}ms", elapsedMs < STAGE_MS / 2)
        assertTrue("staging must actually have started", started.await(5, TimeUnit.SECONDS))

        // The same run keeps going; a later engine build joins it rather than
        // extracting the tree a second time.
        val second = LocalAppRuntimeAssets.prepareWithin(10_000) { error("must not re-stage") }
        assertEquals(STAGED_ROOT, second)
        assertEquals(1, runs.get())

        // Memoised once it succeeded.
        assertEquals(STAGED_ROOT, LocalAppRuntimeAssets.prepareWithin(0) { error("must not re-stage") })
    }

    /**
     * The failure this file exists for: the caller that timed out has already
     * handed `null` to the engine, and the engine cannot take a root later. So
     * the surface must be able to say WHICH of the two no-runtime states it is
     * in — still extracting, or nothing to extract — rather than leaving the
     * user with the engine's "stage local-app-runtime first".
     */
    @Test
    fun `a timed-out cold launch reads as staging, not as a missing runtime`() {
        val release = CountDownLatch(1)

        assertNull(
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        assertEquals(LocalAppRuntimeStaging.Staging, LocalAppRuntimeAssets.stagingStatus())

        release.countDown()

        // Nothing calls `prepare` again here, which is the production reality:
        // it has exactly one call site and no completion callback. The status
        // must still notice the run finished, and must say a restart is what
        // makes the staged tree reachable.
        assertEquals(
            LocalAppRuntimeStaging.StagedAfterNullHandout,
            awaitStatus { it != LocalAppRuntimeStaging.Staging },
        )
    }

    /**
     * A run that yields nothing (asset absent, extraction failed) is a
     * different message from a run still in progress: no amount of waiting
     * helps, so it must not be reported as "still preparing".
     */
    @Test
    fun `a run that produces no runtime reads as unavailable and still retries`() {
        val runs = AtomicInteger()

        assertNull(LocalAppRuntimeAssets.prepareWithin(10_000) { runs.incrementAndGet(); null })
        assertEquals(LocalAppRuntimeStaging.Unavailable, LocalAppRuntimeAssets.stagingStatus())

        // The empty result is not memoised, so the next engine build re-stages
        // — and once that succeeds the status stops claiming it is unavailable.
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000) { runs.incrementAndGet(); STAGED_ROOT },
        )
        assertEquals(2, runs.get())
        assertEquals(
            LocalAppRuntimeStaging.StagedAfterNullHandout,
            LocalAppRuntimeAssets.stagingStatus(),
        )
    }

    /** The warm path: the engine gets the root, so there is nothing to say. */
    @Test
    fun `a runtime staged before the first caller reads as ready and adds no notice`() {
        assertEquals(STAGED_ROOT, LocalAppRuntimeAssets.prepareWithin(10_000) { STAGED_ROOT })

        assertEquals(LocalAppRuntimeStaging.Ready, LocalAppRuntimeAssets.stagingStatus())
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeAssets.stagingStatus()))
        assertEquals(ENGINE_DETAIL, LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true))
        assertNull(LocalAppRuntimeAssets.generationDetail(null, failed = true))
    }

    /**
     * Idle is the JVM/unit-host case (nothing ever asked for the runtime) and
     * must stay silent; the three no-runtime states must each say something,
     * and something different, or the surface cannot tell them apart.
     */
    @Test
    fun `each no-runtime state carries its own explanation and the others carry none`() {
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Idle))
        assertNull(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Ready))

        val notices = listOf(
            LocalAppRuntimeStaging.Staging,
            LocalAppRuntimeStaging.StagedAfterNullHandout,
            LocalAppRuntimeStaging.Unavailable,
        ).map { LocalAppRuntimeAssets.noticeFor(it) }

        notices.forEach { assertNotNull(it) }
        assertTrue("every explanation must be non-empty", notices.all { !it.isNullOrBlank() })
        assertEquals("the three states must not share wording", 3, notices.toSet().size)
    }

    /**
     * The engine's own text stays first — the runtime is not necessarily the
     * only thing that went wrong — and only a FAILED job is annotated, so a job
     * still progressing is not decorated with a failure explanation.
     */
    @Test
    fun `only a failed generation job carries the runtime explanation`() {
        val release = CountDownLatch(1)
        assertNull(
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        val notice = LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Staging)!!

        assertEquals(ENGINE_DETAIL, LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = false))

        val annotated = LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true)
        assertNotNull(annotated)
        assertTrue("the engine's own message must survive", annotated!!.startsWith(ENGINE_DETAIL))
        assertTrue("the runtime state must be explained", annotated.contains(notice))

        // A failure the engine did not describe still gets the explanation.
        assertEquals(notice, LocalAppRuntimeAssets.generationDetail(null, failed = true))

        release.countDown()
    }

    /**
     * [LocalAppRuntimeStaging.StagedAfterNullHandout] latches on "a caller was
     * handed null", which is NOT "the live engine holds no root": the first
     * engine build can fail before it reaches `profile_apps`
     * (`build_mobile_inner_with_ask` is `?`-propagated at host.rs:5422-5430,
     * ahead of :5447), so nothing is memoised, and the reconnect that follows
     * takes the staged root and generates apps normally. The flag cannot be
     * cleared on that evidence — a later `prepare` returning the root does not
     * un-memoise a broker that already took `null` — so the copy must not
     * diagnose the failure or hand the user a restart as THE fix. It must give
     * them the discriminator instead, and this test pins that shape: a
     * condition, and both of its branches.
     */
    @Test
    fun `the staged-late notice states a condition instead of diagnosing the failure`() {
        stageAfterANullHandout()
        assertEquals(
            LocalAppRuntimeStaging.StagedAfterNullHandout,
            LocalAppRuntimeAssets.stagingStatus(),
        )

        val annotated = LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true)!!
        assertTrue("the engine's own message must survive", annotated.startsWith(ENGINE_DETAIL))
        assertFalse(
            "the notice must not assert that the live engine predates the runtime",
            annotated.contains("本次启动的引擎是在它就绪之前建立的"),
        )
        assertTrue("the restart must be offered under a condition", annotated.contains("如果"))
        assertTrue("and the other branch must be named", annotated.contains("否则"))
        assertTrue("the remedy must still be reachable", annotated.contains("重启"))

        // The two states that ARE certain keep their unconditional wording:
        // both are read with no staged root, so the live engine has none.
        assertFalse(
            LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Staging)!!.contains("如果"),
        )
        assertFalse(
            LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Unavailable)!!.contains("如果"),
        )
    }

    /**
     * [LocalAppRuntimeStaging.StagedAfterNullHandout] latches for the whole
     * process and cannot be cleared (see the test above), so keying the notice
     * on the STATE alone pinned a restart instruction to every later failure in
     * a process where the runtime is staged and apps generate normally — an LLM
     * parse error, a validation failure, anything. Whether staging is implicated
     * has to be read off THAT failure, and the only evidence available is the
     * engine's own text.
     */
    @Test
    fun `a staged-late process annotates only the failure that names the runtime`() {
        stageAfterANullHandout()
        assertEquals(
            LocalAppRuntimeStaging.StagedAfterNullHandout,
            LocalAppRuntimeAssets.stagingStatus(),
        )

        // Untouched — not merely un-annotated: no notice and no blank-line join.
        assertEquals(LLM_DETAIL, LocalAppRuntimeAssets.generationDetail(LLM_DETAIL, failed = true))
        // A failure the engine did not describe is not evidence against the
        // runtime either, and must not be blamed on it.
        assertNull(LocalAppRuntimeAssets.generationDetail(null, failed = true))

        // The failure this notice exists for still carries it.
        val annotated = LocalAppRuntimeAssets.generationDetail(ENGINE_DETAIL, failed = true)!!
        assertTrue("the engine's own message must survive", annotated.startsWith(ENGINE_DETAIL))
        assertTrue(
            "a runtime failure must still be explained",
            annotated.contains(
                LocalAppRuntimeAssets.noticeFor(
                    LocalAppRuntimeStaging.StagedAfterNullHandout,
                )!!,
            ),
        )
    }

    /**
     * The gate above is scoped to the one uncertain state, and must not leak
     * into the two certain ones. While [LocalAppRuntimeStaging.Staging] or
     * [LocalAppRuntimeStaging.Unavailable] is readable, no root is staged — and
     * a staged root only ever goes absent -> present — so it was absent when
     * this process's engine was built too. Whatever the proximate cause of a
     * given failure, the retry the user would otherwise reach for is guaranteed
     * to die at Building, so both states keep annotating everything.
     */
    @Test
    fun `a process with no staged root annotates a failure that never names the runtime`() {
        assertNull(LocalAppRuntimeAssets.prepareWithin(10_000) { null })
        assertEquals(LocalAppRuntimeStaging.Unavailable, LocalAppRuntimeAssets.stagingStatus())
        assertTrue(
            "an unavailable runtime must still explain an unrelated failure",
            LocalAppRuntimeAssets.generationDetail(LLM_DETAIL, failed = true)!!
                .contains(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Unavailable)!!),
        )

        val release = CountDownLatch(1)
        assertNull(
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        assertEquals(LocalAppRuntimeStaging.Staging, LocalAppRuntimeAssets.stagingStatus())
        assertTrue(
            "a still-staging runtime must still explain an unrelated failure",
            LocalAppRuntimeAssets.generationDetail(LLM_DETAIL, failed = true)!!
                .contains(LocalAppRuntimeAssets.noticeFor(LocalAppRuntimeStaging.Staging)!!),
        )
        release.countDown()
    }

    /**
     * The process this file exists for: a caller times out and takes `null`, the
     * run finishes anyway, and the NEXT engine build gets the root — so local
     * apps may be perfectly healthy while the flag stays latched.
     */
    private fun stageAfterANullHandout() {
        val release = CountDownLatch(1)
        assertNull(
            LocalAppRuntimeAssets.prepareWithin(BUDGET_MS) {
                assertTrue(release.await(10, TimeUnit.SECONDS))
                STAGED_ROOT
            },
        )
        release.countDown()
        assertEquals(
            STAGED_ROOT,
            LocalAppRuntimeAssets.prepareWithin(10_000) { error("must not re-stage") },
        )
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

    companion object {
        const val BUDGET_MS = 150L
        const val STAGE_MS = 2_000L
        const val STAGED_ROOT = "/data/user/0/com.lingxi.code/files/local-app-runtime"
        /**
         * A real failed-job `detail`, not a paraphrase — the notice is now gated
         * on this text, so a fixture that only resembles it would prove nothing.
         *
         * The clause is `LocalAppsHostBroker::fixed_runtime_mount`
         * (engine-mobile/src/local_apps_host.rs:241); the prefix is
         * `AppError::NotYetAvailable`'s `#[error("not yet available: {0}")]`
         * (local-apps/src/error.rs:64), which is what the fixed Next build's
         * `.map_err(AppError::NotYetAvailable)` produces and what `fail_job`
         * writes into `last_error` verbatim. Preview start reaches the same
         * clause under `io error: ` instead; both are covered by keying on the
         * clause rather than on either prefix.
         */
        const val ENGINE_DETAIL =
            "not yet available: verified local-app Node runtime is unavailable; " +
                "stage local-app-runtime first"
        const val LLM_DETAIL = "生成失败：模型返回的方案无法解析，请重试。"

        /**
         * The executable half of the @After above, and the only place the leak
         * is observable from inside this class (@Before hides it from every
         * method here, and the class that would see it lives in another file).
         * Drop the @After and the last method to run leaves a staged root, so
         * this reads StagedAfterNullHandout and the class goes red — instead of
         * some later class failing for a reason that has nothing to do with it.
         */
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
