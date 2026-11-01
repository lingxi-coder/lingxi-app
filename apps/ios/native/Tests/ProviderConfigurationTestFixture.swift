import Foundation
@testable import LingxiCode

/// Explicit current catalog, never a legacy startup fallback or a real credential read.
@MainActor
func providerConfigurationTestRepository(persistenceURL: URL, accounts: ProviderAccountRepository? = nil) -> ProviderRepository {
    let repository = ProviderRepository(persistenceURL: persistenceURL, accountRepository: accounts ?? ProviderAccountRepository())
    repository.acceptCatalog(Presets.llm.filter { $0.id != "custom" }.map { preset in
        let protocolName = switch preset.id {
        case "anthropic": "anthropic_messages"
        case "google": "gemini_generate_content"
        case "openai", "openai-chatgpt": "open_ai_responses"
        default: "open_ai_chat_completions"
        }
        let env = ["anthropic": "ANTHROPIC_API_KEY", "openai": "OPENAI_API_KEY", "deepseek": "DEEPSEEK_API_KEY", "kimi": "MOONSHOT_API_KEY", "kimi-code": "KIMI_API_KEY", "google": "GEMINI_API_KEY", "openrouter": "OPENROUTER_API_KEY"][preset.id]
        return ProviderCatalogEntry(id: preset.id, displayName: preset.name, baseURL: preset.defaultUrl,
            protocolName: protocolName, authName: preset.id == "openai-chatgpt" ? "chat_gpt_o_auth" : "api_key",
            credentialEnv: env, models: preset.models, modelDetails: [:])
    })
    return repository
}
