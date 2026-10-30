# Local App Market integration

`UnifiedAppMarket` installs schema 2 source releases into `UnifiedLocalApps` and publishes local releases. Native catalog search and pagination have been removed.

The Apps toolbar opens `market.browseURL` in the system default browser. The URL selects `platform=ios`. Website navigation preserves the selected platform; the browser filter does not authorize a local installation.

`lingxi://market/<appId>` opens `MarketInstallPage`, which requests a compatible distribution using `UnifiedAppMarket.targets`, displays the release and native consent, and imports the verified source. Market IDs and local installation IDs are distinct. Existing installations open their local details without being duplicated or updated by the link.

Only iOS targets are accepted. Store advertises web execution; Full also advertises mobile-linux. Target selection includes architecture, runtime version, channel and supported capabilities. The client rejects distributions for another platform, environment or channel before download. Source and release digests remain verified before import.

Application pages still open through the existing Local App browser delivery flow. Installation does not bypass release permissions or transfer application business data between platforms.

Run `LocalAppMarketTests` and `LocalAppDistributionChannelTests` against StoreDebug and FullDebug when the generated bindings and native frameworks are available. Source-only transport checks do not establish full-app or device acceptance.
