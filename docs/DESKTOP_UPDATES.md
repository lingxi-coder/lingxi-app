# Desktop update reminders

Packaged Desktop checks `https://lingxios.net/desktop-updates.json` 20 seconds
after launch and every six hours. Waking from sleep also checks when the last
attempt was at least six hours ago. Development runs do not poll the network.
The URL is configured in `config/product-config.json`.

A newer stable version with an exact `process.platform` / `process.arch` match
shows a blue download button at the bottom of the navigation rail. Opening it
shows the installed and available versions. Download opens the package URL in
the user's default browser. Installation remains manual for the existing
ZIP / tar.gz distribution. Nothing quits the app or interrupts active tasks.
“Remind me tomorrow” hides that version for 24 hours, including across restarts;
a newer version can still appear. Closing the popover keeps the button visible.
Network and malformed-manifest failures are recorded in Desktop diagnostics
without presenting an error popup or clearing a previously discovered update.

## Publishing

The website serves `apps/web/public/desktop-updates.json`. Its initial empty
`releases` array intentionally advertises no packages. Deploy the website with
this file before shipping a Desktop build that reads it. Add a release entry
only after the corresponding package is uploaded and its download verified:

```json
{
  "schemaVersion": 1,
  "releases": [
    {
      "version": "0.2.0",
      "platform": "darwin",
      "arch": "arm64",
      "downloadUrl": "https://lingxios.net/packages/0.2.0/darwin-arm64.zip"
    }
  ]
}
```

The URL above illustrates the schema; it does not identify a published package.
Use the real HTTPS download URL. Current packaging targets are `darwin/arm64`,
`darwin/x64`, `win32/x64` and `linux/x64`. Each can publish independently; missing
platforms never receive a link to another platform's package. Use the actual
Desktop `apps/electron/package.json` version and bump it for each release.
Versions follow SemVer; prerelease entries, equal/older versions, invalid URLs
and packages for other architectures are ignored. Build metadata does not
affect precedence.

The feed requires HTTPS, does not follow redirects, omits credentials, has a
15-second timeout and a 256 KiB body limit. The main process owns the URL and
the selected package. Only registered top-level Desktop windows may read or
operate its IPC API. The renderer cannot supply a URL to open. Reminder
preferences are stored separately from application data in
`desktop-update-reminder.json` under Electron's `userData` directory.
