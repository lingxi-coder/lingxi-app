package com.lingxi.code.settings

import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ProviderAccountsTest {
    @Test fun healthNeverReordersUserPriorityAndOldIdentityHealthIsIgnored() {
        val state = decodeProviderAccounts(snapshot())
        assertEquals(listOf("A", "B"), state.accounts.map { it.accountId })
        assertEquals("cooling_down", state.accounts[0].status)
        assertEquals("unknown", state.accounts[1].status)
        assertFalse(snapshot().contains("apiKey"))
    }

    @Test fun keyIsSeparateFromJsonAndMutationUsesCurrentRevision() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repository.refresh()
        assertNull(repository.command("set_credential", JSONObject().put("accountId", "B"), "separate-secret"))
        assertEquals("separate-secret", source.secret)
        assertFalse(source.request!!.contains("separate-secret"))
        assertEquals(9, JSONObject(source.request!!).getInt("expectedRevision"))
        assertEquals("B", JSONObject(source.request!!).getString("accountId"))
    }

    @Test fun readOnlyRefreshNeverSendsMutation() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repository.refresh()
        repository.refresh()
        assertNull(source.request)
        assertTrue(repository.state.value.loaded)
    }

    @Test fun replacedEngineCannotPublishItsLateSnapshot() = runTest {
        val source = Source()
        val gate = CompletableDeferred<Unit>()
        source.gate = gate
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        val oldRead = launch { repository.refresh() }
        runCurrent()
        repository.bindSource(Source("new-scope"))
        repository.refresh()
        gate.complete(Unit)
        oldRead.join()
        assertEquals("new-scope", repository.state.value.storeScope)
    }

    @Test fun deviceCodeIsShownOnlyForBoundAccountAndOfficialHttpsSite() = runTest {
        for (url in listOf("http://auth.openai.com/codex/device", "https://auth.openai.com.evil.test/codex/device", "https://user@auth.openai.com/codex/device")) {
            val source = Source().apply { verificationUrl = url }
            val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
            var shown = false
            assertNotNull(repository.login(repository.state.value.accounts.first()) { shown = true })
            assertFalse(shown)
            assertEquals(listOf("begin_oauth", "cancel_oauth"), source.requests.map { JSONObject(it).getString("action") })
        }
        val wrongAccount = Source().apply { oauthAccountId = "B" }
        val repository = ProviderAccountsRepository().apply { bindSource(wrongAccount); refresh() }
        assertNotNull(repository.login(repository.state.value.accounts.first()) { fail("wrong account code was shown") })
        assertEquals("cancel_oauth", JSONObject(wrongAccount.requests.last()).getString("action"))
    }

    @Test fun replacingEngineCancelsAndRevokesOnlyOriginalDeviceFlow() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val oldLogin = launch { repository.login(repository.state.value.accounts.first()) {} }
        runCurrent()
        val next = Source("new-scope")
        repository.bindSource(next)
        repository.refresh()
        oldLogin.join()
        assertTrue(oldLogin.isCancelled)
        assertEquals("cancel_oauth", JSONObject(source.requests.last()).getString("action"))
        assertTrue(next.requests.isEmpty())
        assertEquals("new-scope", repository.state.value.storeScope)
    }

    @Test fun lateDevicePollCannotPublishAfterEngineReplacement() = runTest {
        val pollReply = CompletableDeferred<Unit>()
        val source = Source().apply { pollGate = pollReply }
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val login = launch { repository.login(repository.state.value.accounts.first()) {} }
        runCurrent()
        advanceTimeBy(1_000)
        runCurrent()
        assertEquals("poll_oauth", JSONObject(source.requests.last()).getString("action"))
        val next = Source("next-scope")
        repository.bindSource(next)
        repository.refresh()
        pollReply.complete(Unit)
        login.join()
        assertTrue(login.isCancelled)
        assertEquals("next-scope", repository.state.value.storeScope)
        assertTrue(next.requests.isEmpty())
        assertEquals("cancel_oauth", JSONObject(source.requests.last()).getString("action"))
    }

    @Test fun concurrentSignInCannotReplaceActiveAccountDeviceFlow() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val first = launch { repository.login(repository.state.value.accounts.first()) {} }
        runCurrent()
        assertTrue(repository.login(repository.state.value.accounts.last()) {}!!.contains("already active"))
        assertEquals(1, source.requests.size)
        first.cancelAndJoin()
    }

    @Test fun scopedHealthKeepsHostScopeAndIgnoresReplacedIdentity() {
        val json = JSONObject(snapshot()).put("scopedHealth", org.json.JSONArray("""[
          {"accountId":"A","identityGeneration":1,"scopeKind":"model","scopeId":"denied-model","reason":"model_permission_denied"},
          {"accountId":"B","identityGeneration":2,"scopeKind":"endpoint","scopeId":"https://provider.test","reason":"rate_limited","unavailableUntilMs":12000},
          {"accountId":"B","identityGeneration":1,"scopeKind":"model","scopeId":"old-model","reason":"rate_limited"}
        ]"""))
        val state = decodeProviderAccounts(json.toString())
        assertEquals(listOf("model", "endpoint"), state.scopedHealth.map { it.scopeKind })
        assertEquals(listOf("denied-model", "https://provider.test"), state.scopedHealth.map { it.scopeId })
        assertEquals(12000L, state.scopedHealth.last().unavailableUntilMs)
        assertEquals(listOf("A", "B"), state.accounts.map { it.accountId })
        assertTrue(state.modelAvailability.isEmpty())
    }

    @Test fun nextReadOnlyHostSnapshotRestoresEligibilityAfterCooldown() = runTest {
        val source = Source().apply {
            snapshotFactory = {
                val recovered = testScheduler.currentTime >= 15_000
                JSONObject(snapshot()).put("health", org.json.JSONArray().put(JSONObject()
                    .put("accountId", "A").put("identityGeneration", 1)
                    .put("status", if (recovered) "ready" else "cooling_down").put("cooldownUntilMs", 15_000)))
                    .put("modelAvailability", org.json.JSONArray().put(JSONObject()
                        .put("providerId", "openai").put("executionBindingId", "openai-chatgpt").put("modelId", "model")
                        .put("eligibleAccountIds", org.json.JSONArray(if (recovered) listOf("A") else emptyList<String>()))
                        .put("unverifiedAccountIds", org.json.JSONArray(if (recovered) listOf("A") else emptyList<String>()))))
                    .toString()
            }
        }
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        assertEquals(ProviderAccountAvailabilityProblem.CoolingDown, repository.state.value.availabilityProblem)
        assertTrue(repository.state.value.modelAvailability.none { it.hasEligibleAccounts })
        advanceTimeBy(15_000)
        // The foreground timer performs this metadata read, without a business call.
        repository.refresh()
        assertNull(repository.state.value.availabilityProblem)
        assertTrue(repository.state.value.modelAvailability.single().hasEligibleAccounts)
        assertEquals(listOf("A", "B"), repository.state.value.accounts.map { it.accountId })
        assertEquals(2, source.readCount)
        assertTrue(source.requests.isEmpty())
    }

    @Test fun existingAccountsShowAuthenticationReasonInsteadOfFirstSetup() {
        val json = JSONObject(snapshot()).put("health", org.json.JSONArray("""[
          {"accountId":"A","identityGeneration":1,"status":"needs_auth"},
          {"accountId":"B","identityGeneration":2,"status":"needs_auth"}
        ]"""))
        val state = decodeProviderAccounts(json.toString())
        assertEquals(ProviderAccountAvailabilityProblem.NeedsAuthentication, state.availabilityProblem)
        assertNull(state.copy(accounts = emptyList()).availabilityProblem)
        assertNull(ProviderAccountsState().availabilityProblem)
    }

    @Test fun accountModelObservationsAreIdentityFencedAndHostAvailabilityStaysAuthoritative() {
        val json = JSONObject(snapshot())
            .put("modelEntitlements", org.json.JSONArray("""[
              {"accountId":"A","identityGeneration":1,"modelId":"oauth-only","status":"confirmed","observedAtMs":10},
              {"accountId":"B","identityGeneration":2,"modelId":"api-only","status":"forbidden","observedAtMs":5,"retryAtMs":6},
              {"accountId":"A","identityGeneration":99,"modelId":"stale-model","status":"confirmed","observedAtMs":10}
            ]"""))
            .put("modelAvailability", org.json.JSONArray("""[
              {"providerId":"openai","executionBindingId":"openai-chatgpt","modelId":"oauth-only","eligibleAccountIds":["A"],"confirmedAccountIds":["A"],"unverifiedAccountIds":[],"forbiddenAccountIds":[]},
              {"providerId":"openai","executionBindingId":"openai","modelId":"api-only","eligibleAccountIds":["B"],"confirmedAccountIds":[],"unverifiedAccountIds":["B"],"forbiddenAccountIds":[]},
              {"providerId":"openai","executionBindingId":"openai","modelId":"denied","eligibleAccountIds":[],"confirmedAccountIds":[],"unverifiedAccountIds":[],"forbiddenAccountIds":["B"]}
            ]"""))
        val state = decodeProviderAccounts(json.toString())
        assertEquals(listOf("oauth-only", "api-only"), state.modelEntitlements.map { it.modelId })
        assertEquals("forbidden", state.modelEntitlements.last().status)
        // The Host already applies TTL, billing, health and model eligibility.
        assertEquals(listOf("confirmed", "unverified", "forbidden"), state.modelAvailability.map { it.permissionStatus })
        assertEquals(listOf("openai-chatgpt", "openai", "openai"), state.modelAvailability.map { it.executionBindingId })
        assertEquals(listOf("A", "B"), state.accounts.map { it.accountId })
    }

    @Test fun absentModelEvidenceNeverClaimsConfirmedPermission() {
        val state = decodeProviderAccounts(snapshot())
        assertTrue(state.modelEntitlements.isEmpty())
        assertTrue(state.modelAvailability.isEmpty())
        val undiscovered = ProviderAccountModelAvailability("openai", "openai", "model", emptyList(), emptyList(), emptyList(), emptyList())
        assertEquals("unverified", undiscovered.permissionStatus)
    }

    @Test fun actualRoutingSelectionIsShownWithoutReorderingAndOldIdentityEventsAreIgnored() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repository.refresh()
        repository.recordRouting(source, """{"status":"selected","selectedAccountId":"B","identityGeneration":2,"model":"model-id"}""")
        assertEquals("B", repository.state.value.lastSelectedAccountId)
        assertEquals(listOf("A", "B"), repository.state.value.accounts.map { it.accountId })
        repository.refresh()
        assertEquals("B", repository.state.value.lastSelectedAccountId)
        repository.recordRouting(source, """{"status":"selected","selectedAccountId":"A","identityGeneration":99,"model":"old"}""")
        assertEquals("B", repository.state.value.lastSelectedAccountId)
        repository.bindSource(Source("other-host"))
        repository.recordRouting(source, """{"status":"all_unavailable"}""")
        assertFalse(repository.state.value.allAccountsUnavailable)
    }

    @Test fun billingDefaultsToCompatibleOnlyAndConsentTargetsOneProviderWithRevision() = runTest {
        assertFalse(decodeProviderAccounts(snapshot()).providerPolicies["openai"]!!.allowSubscriptionToMetered)
        val withoutPolicy = JSONObject(snapshot()).apply { remove("providerPolicies") }.toString()
        assertTrue(decodeProviderAccounts(withoutPolicy).providerPolicies.isEmpty())
        val source = Source()
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repository.refresh()
        assertNull(repository.setBillingConsent("openai", true))
        val request = JSONObject(source.request!!)
        assertEquals("set_policy", request.getString("action"))
        assertEquals("openai", request.getString("providerId"))
        assertEquals(9, request.getInt("expectedRevision"))
        assertTrue(request.getBoolean("allowSubscriptionToMetered"))
        assertNull(source.secret)
        assertTrue(repository.state.value.providerPolicies["openai"]!!.allowSubscriptionToMetered)
        assertNull(repository.setBillingConsent("openai", false))
        assertFalse(repository.state.value.providerPolicies["openai"]!!.allowSubscriptionToMetered)
        assertEquals(listOf("A", "B"), repository.state.value.accounts.map { it.accountId })
    }

    @Test fun remoteOAuthCannotAccidentallyStartALocalOrTargetHostFlow() = runTest {
        val source = Source(oauthSupported = false)
        val repository = ProviderAccountsRepository()
        repository.bindSource(source)
        repository.refresh()
        assertFalse(repository.state.value.oauthSupported)
        assertTrue(repository.login(repository.state.value.accounts.first()) {}!!.contains("execution host"))
        assertTrue(source.requests.isEmpty())
    }

    @Test fun nativeFailureBodiesAndSecretsAreNeverForwardedToUi() {
        assertEquals("safe fallback", providerAccountError(Exception("provider body contains secret-value"), "safe fallback"))
        assertFalse(providerAccountError(Exception("authentication rejected secret-value"), "fallback").contains("secret-value"))
        assertTrue(providerAccountError(Exception("different upstream account"), "fallback").contains("new account"))
    }

    @Test fun deviceLoginPollsOnlyItsAccountAndHonorsHostInterval() = runTest {
        val source = Source().apply { pendingPolls = 1 }
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val started = testScheduler.currentTime
        assertNull(repository.login(repository.state.value.accounts.first()) { authorization ->
            assertEquals("A", authorization.accountId)
            assertEquals("ABCD-EFGH", authorization.userCode)
            assertEquals("https://auth.openai.com/codex/device", authorization.verificationUrl)
        })
        assertEquals(4_000L, testScheduler.currentTime - started)
        assertEquals(listOf("begin_oauth", "poll_oauth", "poll_oauth"), source.requests.map { JSONObject(it).getString("action") })
        assertTrue(source.requests.all { JSONObject(it).getString("accountId") == "A" })
        assertEquals("flow-A", JSONObject(source.requests.last()).getString("flowId"))
        assertTrue(source.requests.none { JSONObject(it).has("callbackUrl") || JSONObject(it).has("providerId") })
    }

    @Test fun cancelledLoginRevokesOriginalHostDeviceFlow() = runTest {
        val source = Source()
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val login = launch { repository.login(repository.state.value.accounts.first()) {} }
        runCurrent()
        login.cancelAndJoin()
        assertEquals(listOf("begin_oauth", "cancel_oauth"), source.requests.map { JSONObject(it).getString("action") })
        assertEquals("flow-A", JSONObject(source.requests.last()).getString("flowId"))
    }

    @Test fun timedOutDeviceFlowIsTerminalAndRevokesHostFlow() = runTest {
        val source = Source().apply { pendingPolls = Int.MAX_VALUE }
        val repository = ProviderAccountsRepository().apply { bindSource(source); refresh() }
        val login = async { repository.login(repository.state.value.accounts.first()) {} }
        runCurrent()
        advanceTimeBy(900 * 1000L)
        runCurrent()
        assertTrue(login.await()!!.contains("timed out"))
        assertEquals("cancel_oauth", JSONObject(source.requests.last()).getString("action"))
    }

    private class Source(private val scope: String = "scope", private val oauthSupported: Boolean = true) : ConversationSource {
        override val supportsProviderAccountOAuth: Boolean get() = oauthSupported
        val requests = mutableListOf<String>()
        var request: String? = null
        var secret: String? = null
        var gate: CompletableDeferred<Unit>? = null
        var allowMetered = false
        var readCount = 0
        var snapshotFactory: (() -> String)? = null
        var pendingPolls = 0
        var pollGate: CompletableDeferred<Unit>? = null
        var verificationUrl = "https://auth.openai.com/codex/device"
        var oauthAccountId = "A"
        override suspend fun providerAccountsSnapshot(): String { gate?.await(); readCount += 1; return snapshotFactory?.invoke() ?: snapshot(scope, allowMetered) }
        override suspend fun providerAccountCommand(requestJson: String, credential: String?): String {
            request = requestJson; requests += requestJson; secret = credential
            if (JSONObject(requestJson).getString("action") == "set_policy") allowMetered = JSONObject(requestJson).getBoolean("allowSubscriptionToMetered")
            return when (JSONObject(requestJson).getString("action")) {
                "begin_oauth" -> JSONObject().put("mode", "device_code").put("flow_id", "flow-A")
                    .put("account_id", oauthAccountId).put("verification_url", verificationUrl)
                    .put("user_code", "ABCD-EFGH").put("interval_seconds", 1).put("expires_in_seconds", 900).toString()
                "poll_oauth" -> {
                    // Model a native transport that returns a response after cancellation.
                    pollGate?.let { withContext(NonCancellable) { it.await() } }
                    if (pendingPolls-- > 0) """{"status":"pending","retry_after_seconds":3}"""
                    else JSONObject().put("status", "completed").put("retry_after_seconds", 0)
                        .put("snapshot", JSONObject(snapshot(scope, allowMetered))).toString()
                }
                else -> snapshot(scope, allowMetered)
            }
        }
    }

    companion object {
        private fun snapshot(scope: String = "scope", allowMetered: Boolean = false) = """{
            "storeScope":"$scope","configRevision":9,
            "providerPolicies":{"openai":{"allowSubscriptionToMetered":$allowMetered}},
            "accounts":[
              {"accountId":"B","providerId":"openai","displayName":"Second","authKind":"api_key","executionBindingId":"openai","enabled":true,"userPriority":2,"identityGeneration":2},
              {"accountId":"A","providerId":"openai","displayName":"First","authKind":"oauth","executionBindingId":"openai-chatgpt","enabled":true,"userPriority":1,"identityGeneration":1}
            ],
            "health":[{"accountId":"A","identityGeneration":1,"status":"cooling_down","cooldownUntilMs":1234},
                      {"accountId":"B","identityGeneration":1,"status":"needs_auth"}]
        }"""
    }
}
