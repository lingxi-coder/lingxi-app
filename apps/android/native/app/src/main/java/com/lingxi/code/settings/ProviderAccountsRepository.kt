package com.lingxi.code.settings

import com.lingxi.code.conversation.ConversationSource
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.job
import kotlinx.coroutines.delay
import java.net.URI
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicLong

data class ProviderAccountRow(
    val accountId: String,
    val providerId: String,
    val displayName: String,
    val authKind: String,
    val executionBindingId: String,
    val enabled: Boolean,
    val userPriority: Long,
    val identityGeneration: Long,
    val status: String,
    val reason: String?,
    val cooldownUntilMs: Long?,
)

data class ProviderDeviceAuthorization(val accountId: String, val userCode: String, val verificationUrl: String, val expiresInSeconds: Long)

data class ProviderAccountPolicy(val allowSubscriptionToMetered: Boolean = false)

data class ProviderAccountModelEntitlement(
    val accountId: String,
    val identityGeneration: Long,
    val modelId: String,
    val status: String,
    val observedAtMs: Long,
    val retryAtMs: Long?,
)

data class ProviderAccountScopedHealth(
    val accountId: String,
    val identityGeneration: Long,
    val scopeKind: String,
    val scopeId: String,
    val unavailableUntilMs: Long?,
    val reason: String,
)

data class ProviderAccountModelAvailability(
    val providerId: String,
    val executionBindingId: String,
    val modelId: String,
    val eligibleAccountIds: List<String>,
    val confirmedAccountIds: List<String>,
    val unverifiedAccountIds: List<String>,
    val forbiddenAccountIds: List<String>,
) {
    val hasEligibleAccounts: Boolean get() = eligibleAccountIds.isNotEmpty()

    /** Presentation of the Host projection; no native account eligibility or routing decisions. */
    val permissionStatus: String get() = when {
        confirmedAccountIds.isNotEmpty() -> "confirmed"
        unverifiedAccountIds.isNotEmpty() -> "unverified"
        forbiddenAccountIds.isNotEmpty() -> "forbidden"
        else -> "unverified"
    }
}

internal fun findModelAvailability(rows: List<ProviderAccountModelAvailability>, qualifiedModel: String): ProviderAccountModelAvailability? =
    rows.firstOrNull { "${it.executionBindingId}/${it.modelId}" == qualifiedModel }

enum class ProviderAccountAvailabilityProblem { CoolingDown, NeedsAuthentication, QuotaExhausted, Disabled, StorageUnavailable, ModelForbidden, NoEligibleAccount }

data class ProviderAccountsState(
    val storeScope: String = "",
    val configRevision: Long = 0,
    val accounts: List<ProviderAccountRow> = emptyList(),
    val providerPolicies: Map<String, ProviderAccountPolicy> = emptyMap(),
    val modelEntitlements: List<ProviderAccountModelEntitlement> = emptyList(),
    val modelAvailability: List<ProviderAccountModelAvailability> = emptyList(),
    val scopedHealth: List<ProviderAccountScopedHealth> = emptyList(),
    val error: String? = null,
    val loaded: Boolean = false,
    val bindingGeneration: Long = 0,
    val oauthSupported: Boolean = true,
    val lastSelectedAccountId: String? = null,
    val lastSelectedModel: String? = null,
    val allAccountsUnavailable: Boolean = false,
) {
    fun providerAccounts(bindingId: String): List<ProviderAccountRow> = accounts.filter {
        if (bindingId == "openai" || bindingId == "openai-chatgpt") it.providerId == "openai"
        else it.providerId == bindingId || it.executionBindingId == bindingId || it.executionBindingId.startsWith("$bindingId:")
    }

    fun configurationStatus(bindingId: String): String {
        if (error != null) return "read_failed"
        if (!loaded) return "loading"
        val rows = providerAccounts(bindingId)
        if (rows.isEmpty()) return "unconfigured"
        val enabled = rows.filter { it.enabled }
        if (enabled.isEmpty()) return "disabled"
        if (enabled.any { it.status == "ready" }) return "configured"
        if (enabled.any { it.status == "storage_unavailable" }) return "storage_unavailable"
        if (enabled.any { it.status == "cooling_down" || it.status == "quota_exhausted" }) return "temporarily_unavailable"
        if (enabled.any { it.status == "needs_auth" }) return "needs_auth"
        return "unverified"
    }

    /** Copy only: the Host projection remains the sole model/account eligibility authority. */
    val availabilityProblem: ProviderAccountAvailabilityProblem? get() {
        if (!loaded || accounts.isEmpty() || modelAvailability.any { it.hasEligibleAccounts }) return null
        val enabled = accounts.filter { it.enabled }
        return when {
            error != null || enabled.any { it.status == "storage_unavailable" } -> ProviderAccountAvailabilityProblem.StorageUnavailable
            enabled.isEmpty() -> ProviderAccountAvailabilityProblem.Disabled
            enabled.any { it.status == "cooling_down" } || scopedHealth.any { it.reason == "rate_limited" } -> ProviderAccountAvailabilityProblem.CoolingDown
            enabled.any { it.status == "needs_auth" } -> ProviderAccountAvailabilityProblem.NeedsAuthentication
            enabled.any { it.status == "quota_exhausted" } -> ProviderAccountAvailabilityProblem.QuotaExhausted
            modelAvailability.any { it.forbiddenAccountIds.isNotEmpty() } -> ProviderAccountAvailabilityProblem.ModelForbidden
            else -> ProviderAccountAvailabilityProblem.NoEligibleAccount
        }
    }
}

/** Host-owned account catalogue. This class never persists credentials or picks a request account. */
class ProviderAccountsRepository {
    private val mutations = Mutex()
    private class Binding(val source: ConversationSource)
    @Volatile private var binding: Binding? = null
    private val loginLock = Any()
    private var activeLogin: Job? = null
    private val readTicket = AtomicLong(0)
    private val mutable = MutableStateFlow(ProviderAccountsState())
    val state = mutable.asStateFlow()

    fun bindSource(source: ConversationSource?) {
        if (binding?.source === source) return
        synchronized(loginLock) { activeLogin?.cancel(); activeLogin = null }
        binding = source?.let(::Binding)
        mutable.value = ProviderAccountsState(bindingGeneration = mutable.value.bindingGeneration + 1, oauthSupported = source?.supportsProviderAccountOAuth == true)
    }

    suspend fun refresh() {
        val current = binding ?: return fail("Engine unavailable")
        val ticket = readTicket.incrementAndGet()
        try {
            val snapshot = withTimeout(10_000) { decodeProviderAccounts(current.source.providerAccountsSnapshot()) }
            if (binding === current && ticket == readTicket.get() && snapshot.configRevision >= mutable.value.configRevision) mutable.value = retainUiState(snapshot)
        } catch (_: TimeoutCancellationException) {
            if (binding === current) fail("Account read timed out. Retry.")
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            if (binding === current) fail("Unable to read accounts. Retry after reconnecting the engine.")
        }
    }

    suspend fun command(action: String, fields: JSONObject = JSONObject(), credential: String? = null): String? =
        mutations.withLock {
            val current = binding ?: return@withLock "Engine unavailable"
            try {
                val request = fields.put("action", action)
                    .put("expectedRevision", mutable.value.configRevision)
                    .put("operationId", nextOperation.getAndIncrement())
                withTimeout(20_000) { current.source.providerAccountCommand(request.toString(), credential) }
                check(binding === current) { "engine changed" }
                val ticket = readTicket.incrementAndGet()
                val snapshot = withTimeout(10_000) { decodeProviderAccounts(current.source.providerAccountsSnapshot()) }
                check(binding === current) { "engine changed" }
                if (ticket == readTicket.get() && snapshot.configRevision >= mutable.value.configRevision) mutable.value = retainUiState(snapshot)
                null
            } catch (_: TimeoutCancellationException) {
                if (binding === current) refresh()
                "Account operation timed out. Refresh the list before retrying."
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Exception) {
                if (binding === current) refresh()
                providerAccountError(failure, "Account operation failed. The account list has been refreshed; retry the operation.")
            }
        }

    fun recordRouting(source: ConversationSource, eventJson: String) {
        if (binding?.source !== source) return
        runCatching {
            val event = JSONObject(eventJson)
            when (event.optString("status")) {
                "selected" -> {
                    val accountId = event.getString("selectedAccountId")
                    val account = mutable.value.accounts.firstOrNull { it.accountId == accountId } ?: return
                    val identity = event.getLong("identityGeneration")
                    // A late routing event must not attach to replacement credentials.
                    val snapshotIdentity = account.identityGeneration
                    if (identity != snapshotIdentity) return
                    mutable.value = mutable.value.copy(lastSelectedAccountId = account.accountId,
                        lastSelectedModel = event.optString("model").takeIf { it.isNotBlank() }, allAccountsUnavailable = false)
                }
                "all_unavailable" -> mutable.value = mutable.value.copy(lastSelectedAccountId = null,
                    lastSelectedModel = null, allAccountsUnavailable = true)
            }
        }
    }

    suspend fun setBillingConsent(providerId: String, allowSubscriptionToMetered: Boolean): String? =
        command("set_policy", JSONObject().put("providerId", providerId)
            .put("allowSubscriptionToMetered", allowSubscriptionToMetered))

    suspend fun saveAccount(account: ProviderAccountRow, name: String, credential: String?): String? {
        if (name != account.displayName) {
            command("update", JSONObject().put("accountId", account.accountId).put("displayName", name))?.let { return it }
        }
        return credential?.let { command("set_credential", JSONObject().put("accountId", account.accountId), it) }
    }

    suspend fun login(account: ProviderAccountRow, onDeviceCode: (ProviderDeviceAuthorization) -> Unit): String? {
        val current = binding ?: return "Engine unavailable"
        if (!current.source.supportsProviderAccountOAuth) return "Sign in on the execution host to add or reconnect an OAuth account."
        val login = currentCoroutineContext().job
        synchronized(loginLock) {
            if (activeLogin != null) return "Another sign-in is already active. Cancel it before retrying."
            activeLogin = login
        }
        var flowId: String? = null
        var completed = false
        val expectedRevision = mutable.value.configRevision
        fun request(action: String) = JSONObject().put("action", action).put("accountId", account.accountId)
            .put("expectedRevision", expectedRevision).put("operationId", nextOperation.getAndIncrement())
        suspend fun checkActive() {
            currentCoroutineContext().ensureActive()
            check(binding === current)
            check(synchronized(loginLock) { activeLogin === login })
        }
        try {
            val begin = JSONObject(withTimeout(20_000) {
                current.source.providerAccountCommand(request("begin_oauth").toString(), null)
            })
            // Capture the opaque flow before validating, so malformed responses are revoked too.
            flowId = begin.getString("flow_id")
            checkActive()
            check(begin.getString("account_id") == account.accountId)
            check(begin.getString("mode") == "device_code")
            val url = URI(begin.getString("verification_url"))
            check(url.scheme == "https" && url.host == "auth.openai.com" && url.userInfo == null && url.port == -1 && url.fragment == null)
            val code = begin.getString("user_code").also { check(it.isNotBlank()) }
            val expires = begin.getLong("expires_in_seconds").coerceIn(1L, 900L)
            onDeviceCode(ProviderDeviceAuthorization(account.accountId, code, url.toString(), expires))
            withTimeout(expires * 1_000L) {
                var retrySeconds = begin.getLong("interval_seconds").coerceIn(1L, 900L)
                while (true) {
                    delay(retrySeconds * 1_000L)
                    checkActive()
                    val poll = JSONObject(withTimeout(30_000) {
                        current.source.providerAccountCommand(request("poll_oauth").put("flowId", flowId).toString(), null)
                    })
                    checkActive()
                    when (poll.getString("status")) {
                        "pending" -> retrySeconds = poll.getLong("retry_after_seconds").coerceIn(1L, 900L)
                        "completed" -> {
                            val snapshot = decodeProviderAccounts(poll.getJSONObject("snapshot").toString())
                            check(snapshot.storeScope == mutable.value.storeScope)
                            check(snapshot.accounts.any { it.accountId == account.accountId && it.executionBindingId == account.executionBindingId })
                            readTicket.incrementAndGet()
                            if (snapshot.configRevision >= mutable.value.configRevision) mutable.value = retainUiState(snapshot)
                            completed = true
                            break
                        }
                        else -> error("unexpected device authorization status")
                    }
                }
            }
            refresh()
            return null
        } catch (_: TimeoutCancellationException) {
            return "Sign-in timed out. Retry using the intended account."
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            return providerAccountError(failure, "Sign-in was not completed. Retry using the intended account.")
        } finally {
            if (!completed && flowId != null) {
                kotlinx.coroutines.withContext(kotlinx.coroutines.NonCancellable) {
                    runCatching { withTimeout(5_000) {
                        current.source.providerAccountCommand(request("cancel_oauth").put("flowId", flowId).toString(), null)
                    } }
                }
            }
            synchronized(loginLock) { if (activeLogin === login) activeLogin = null }
        }
    }

    private fun retainUiState(snapshot: ProviderAccountsState): ProviderAccountsState {
        val previous = mutable.value
        val priorSelection = previous.accounts.firstOrNull { it.accountId == previous.lastSelectedAccountId }
        val selectionValid = priorSelection != null && snapshot.accounts.any {
            it.accountId == priorSelection.accountId && it.identityGeneration == priorSelection.identityGeneration
        }
        return snapshot.copy(bindingGeneration = previous.bindingGeneration, oauthSupported = previous.oauthSupported,
            lastSelectedAccountId = previous.lastSelectedAccountId.takeIf { selectionValid },
            lastSelectedModel = previous.lastSelectedModel.takeIf { selectionValid },
            allAccountsUnavailable = previous.allAccountsUnavailable)
    }

    private fun fail(message: String) { mutable.value = mutable.value.copy(error = message) }
    private companion object { val nextOperation = AtomicLong(1) }
}

internal fun decodeProviderAccounts(json: String): ProviderAccountsState {
    val root = JSONObject(json)
    val health = root.optJSONArray("health") ?: JSONArray()
    val byIdentity = (0 until health.length()).associate { index ->
        val item = health.getJSONObject(index)
        (item.getString("accountId") to item.optLong("identityGeneration")) to item
    }
    val accounts = root.getJSONArray("accounts")
    val policies = root.optJSONObject("providerPolicies") ?: JSONObject()
    val identities = (0 until accounts.length()).associate { index ->
        val account = accounts.getJSONObject(index)
        account.getString("accountId") to account.getLong("identityGeneration")
    }
    fun stringList(owner: JSONObject, key: String): List<String> {
        val values = owner.optJSONArray(key) ?: return emptyList()
        return (0 until values.length()).map { values.getString(it) }
    }
    val entitlements = root.optJSONArray("modelEntitlements") ?: JSONArray()
    val availability = root.optJSONArray("modelAvailability") ?: JSONArray()
    val scopedHealth = root.optJSONArray("scopedHealth") ?: JSONArray()
    return ProviderAccountsState(
        scopedHealth = (0 until scopedHealth.length()).mapNotNull { index ->
            val row = scopedHealth.getJSONObject(index)
            val accountId = row.getString("accountId")
            val generation = row.getLong("identityGeneration")
            if (identities[accountId] != generation) null else ProviderAccountScopedHealth(
                accountId, generation, row.getString("scopeKind"), row.getString("scopeId"),
                row.takeUnless { it.isNull("unavailableUntilMs") }?.getLong("unavailableUntilMs"), row.getString("reason"),
            )
        },
        modelEntitlements = (0 until entitlements.length()).mapNotNull { index ->
            val row = entitlements.getJSONObject(index)
            val accountId = row.getString("accountId")
            val generation = row.getLong("identityGeneration")
            if (identities[accountId] != generation) null else ProviderAccountModelEntitlement(
                accountId, generation, row.getString("modelId"), row.getString("status"),
                row.getLong("observedAtMs"), row.takeUnless { it.isNull("retryAtMs") }?.getLong("retryAtMs"),
            )
        },
        modelAvailability = (0 until availability.length()).map { index ->
            val row = availability.getJSONObject(index)
            ProviderAccountModelAvailability(row.getString("providerId"), row.getString("executionBindingId"), row.getString("modelId"),
                stringList(row, "eligibleAccountIds"), stringList(row, "confirmedAccountIds"),
                stringList(row, "unverifiedAccountIds"), stringList(row, "forbiddenAccountIds"))
        },
        storeScope = root.getString("storeScope"),
        configRevision = root.getLong("configRevision"),
        providerPolicies = policies.keys().asSequence().associateWith { providerId ->
            ProviderAccountPolicy(policies.getJSONObject(providerId).optBoolean("allowSubscriptionToMetered", false))
        },
        accounts = (0 until accounts.length()).map { index ->
            val account = accounts.getJSONObject(index)
            val status = byIdentity[account.getString("accountId") to account.optLong("identityGeneration")]
            ProviderAccountRow(
                accountId = account.getString("accountId"), providerId = account.getString("providerId"),
                displayName = account.getString("displayName"), authKind = account.getString("authKind"),
                executionBindingId = account.getString("executionBindingId"), enabled = account.getBoolean("enabled"),
                userPriority = account.getLong("userPriority"), identityGeneration = account.getLong("identityGeneration"), status = status?.optString("status") ?: "unknown",
                reason = status?.optString("reason")?.takeUnless { it.isBlank() || it == "null" },
                cooldownUntilMs = status?.takeUnless { it.isNull("cooldownUntilMs") }?.optLong("cooldownUntilMs"),
            )
        }.sortedWith(compareBy<ProviderAccountRow> { it.userPriority }.thenBy { it.accountId }),
        loaded = true,
    )
}

/** Only recognized public reasons become UI text; never forward a native exception or provider body. */
internal fun providerAccountError(error: Exception, fallback: String): String {
    val detail = error.message.orEmpty()
    return when {
        "different upstream account" in detail -> "This sign-in belongs to a different account. Add it as a new account instead."
        "duplicate upstream" in detail || "already linked" in detail -> "This account is already saved. Reconnect the existing account instead."
        "authentication rejected" in detail -> "Authentication was rejected for this account. Update its API key or sign in again."
        "rate limited" in detail -> "This account is temporarily rate limited. Retry after its cooldown."
        "credential storage" in detail || "credential persistence" in detail -> "Secure storage is unavailable. Reconnect the engine and retry."
        "revision" in detail || "account changed" in detail -> "This account changed during the operation. Review the refreshed list and retry."
        else -> fallback
    }
}
