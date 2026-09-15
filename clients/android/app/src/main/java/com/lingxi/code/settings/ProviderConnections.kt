package com.lingxi.code.settings

import com.lingxi.code.model.GenericProvider
import com.lingxi.code.model.ProviderConnection

/**
 * What can be wrong with a provider's connection list.
 *
 * Every case is something the engine would otherwise discover as a parse error
 * or - for a duplicate id - as a connection silently dropped on the floor.
 */
sealed interface ProviderConnectionProblem {
    /** Empty, or carrying a separator the engine's profile parser splits on. */
    data object MissingId : ProviderConnectionProblem

    data class DuplicateId(val id: String) : ProviderConnectionProblem

    data class InvalidUrl(val id: String) : ProviderConnectionProblem
}

/**
 * The editing and validation rules for a provider reachable several ways.
 *
 * Pure so the rules can be tested without a Compose tree or a live store; the
 * iOS side has the same rules in `ProviderStoredProfile`'s extension, and
 * `connection_group_test.rs` covers what the engine does with the result.
 */
object ProviderConnections {

    /**
     * `:` would produce `group:a:b`, which the engine's connection-profile
     * parser splits at the wrong colon; `/` would break the `profile/model`
     * qualified reference. Both are rejected rather than rewritten, so the id
     * the user typed is the id they see.
     */
    fun isValidId(id: String): Boolean {
        val trimmed = id.trim()
        return trimmed.isNotEmpty() && !trimmed.contains(':') && !trimmed.contains('/')
    }

    /**
     * Add a connection.
     *
     * The FIRST add migrates the flat provider into TWO connections: the
     * endpoint already configured becomes `default` so nothing the user set is
     * lost, and the new one starts empty. Going straight from none to one would
     * rename the engine profile to `group:default` while still describing a
     * single endpoint.
     */
    fun withAddedConnection(provider: GenericProvider): GenericProvider =
        if (provider.connections.isEmpty()) {
            provider.copy(
                connections = listOf(
                    ProviderConnection(id = "default", url = provider.url),
                    ProviderConnection(id = "", url = ""),
                ),
            )
        } else {
            provider.copy(connections = provider.connections + ProviderConnection(id = "", url = ""))
        }

    /**
     * Remove a connection, collapsing back to a flat provider when one is left -
     * a one-entry list would keep claiming several ways to reach this provider
     * and would still cost the `group:id` rename. The survivor's endpoint is
     * lifted to provider level so the reachable URL is unchanged.
     */
    fun withRemovedConnection(provider: GenericProvider, index: Int): GenericProvider {
        if (index !in provider.connections.indices) return provider
        val remaining = provider.connections.filterIndexed { i, _ -> i != index }
        if (remaining.size > 1) return provider.copy(connections = remaining)
        val survivorUrl = remaining.firstOrNull()?.url?.trim().orEmpty()
        return provider.copy(
            url = if (survivorUrl.isEmpty()) provider.url else survivorUrl,
            connections = emptyList(),
        )
    }

    fun withUpdatedConnection(
        provider: GenericProvider,
        index: Int,
        transform: (ProviderConnection) -> ProviderConnection,
    ): GenericProvider {
        if (index !in provider.connections.indices) return provider
        return provider.copy(
            connections = provider.connections.mapIndexed { i, connection ->
                if (i == index) transform(connection) else connection
            },
        )
    }

    /** The first problem, or `null` when the list is usable (empty included). */
    fun validate(provider: GenericProvider): ProviderConnectionProblem? {
        if (provider.connections.isEmpty()) return null
        val seen = mutableSetOf<String>()
        for (connection in provider.connections) {
            val id = connection.id.trim()
            if (!isValidId(id)) return ProviderConnectionProblem.MissingId
            if (!seen.add(id)) return ProviderConnectionProblem.DuplicateId(id)
            // Inheriting the provider URL here would point a connection the user
            // just added at the endpoint they are trying to add one BESIDE, so
            // an endpoint is required rather than inherited.
            val url = connection.url.trim()
            val scheme = url.substringBefore("://", missingDelimiterValue = "").lowercase()
            val host = url.substringAfter("://", missingDelimiterValue = "").substringBefore('/')
            if (scheme !in setOf("http", "https") || host.isEmpty()) {
                return ProviderConnectionProblem.InvalidUrl(id)
            }
        }
        return null
    }
}
