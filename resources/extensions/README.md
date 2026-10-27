# Public extension directory

`public-directory.json` is the single bundled source for the Desktop, Android
and iOS discovery pages. It contains descriptions, upstream links, optional
MCP configuration drafts, and optional plugin marketplace sources. Reading it
does not download, install, enable or authenticate a tool.

MCP entries with `mcp` open the existing configuration editor. Saving a server
means configured, not connected. No credentials belong in this file. Entries
without a configuration draft open the upstream setup instructions.

Plugin entries with `marketplace` use the existing marketplace preview and
confirmation flow. After the engine has fetched and validated the marketplace,
its real catalog supplies the individual plugin installation operation. The
directory is not a replacement for marketplace manifests or their authority.

The installer reads the current Claude `.claude-plugin/plugin.json` and
`.claude-plugin/marketplace.json` formats alongside LingXi's native metadata.
When both formats are present, native metadata has precedence; an invalid
selected file is an error, not a reason to silently select the other format.
Installation, settings, credentials and writable plugin data remain in LingXi's
own storage. This is format interoperability: each plugin's required tools,
executables, browser features and authentication still need to be available.
Upstream plugin content is not rewritten or converted into LingXi-authored
Skills. See the [official manifest reference](https://code.claude.com/docs/en/plugins/manifest-reference)
and [marketplace documentation](https://code.claude.com/docs/en/plugin-marketplaces).

Local App WebMCP support and the directory are independent. The host supplies
the pinned official `document.modelContext` polyfill; an app registers its own
business actions using that standard API. Users choose which browser tools or
plugins can discover and call those actions. Installing a browser MCP does not
automatically grant it access to every Local App or make a browser's native
WebMCP/CDP support available.

Platform lists describe where the discovery action is available. A marketplace
may contain plugins with additional browser, operating system, account or
runtime requirements; those are checked by the individual installation and
configuration flows. Desktop browser MCP process recipes are not offered as
mobile process configurations.

Keep descriptions in `en`, `zh-Hans`, `zh-Hant`, `ja` and `ko`. Verify upstream
sources and actual published package versions before changing executable MCP
recipes. Pin npm package versions rather than using `@latest`; allow only HTTPS
source/documentation links. Do not add secrets, shell scripts, automatic
installation hooks or provider credentials.

Initial sources were checked on 2026-10-25 against the maintained repositories:
ChromeDevTools/chrome-devtools-mcp, microsoft/playwright-mcp,
vercel-labs/agent-browser, upstash/context7, github/github-mcp-server,
lingxi-coder/local-app-builder, ora/webmcp, nekuda-ai/webmcp-kit, and
anthropics/claude-plugins-official. Each entry links to its own source.
