# LingXi Browser

This extension connects the selected supported browser to LingXi Desktop's device
permission settings. The browser profile stays in that browser. Model credentials and approval
decisions stay in LingXi Desktop.

For this development distribution:

1. In LingXi Desktop, open **Settings → Computer use → Browser**, choose the
   browser to connect, then choose **Set up extension**. This registers the
   native messaging host only for that browser and the current macOS user.
2. Open the selected browser's Extensions page, enable Developer mode, choose
   **Load unpacked**, and select the directory shown by **Show extension files**.
   Reload an existing extension after updating Desktop. The directory must remain in place.
3. Return to LingXi and choose **Retry**. Connected means the extension has
   completed the versioned native handshake; registering the native host alone
   does not mean the selected browser loaded the extension.

| Browser | Extensions page | Adapter |
| --- | --- | --- |
| Google Chrome | `chrome://extensions` | Chromium extension |
| Microsoft Edge | `edge://extensions` | Chromium extension |
| Brave | `brave://extensions` | Chromium extension |
| Chromium | `chrome://extensions` | Chromium extension |
| Safari | — | Not available yet |
| Firefox | — | Not available yet |

The adapters use each browser's native-messaging directory. Safari and Firefox
require separate adapters; installing this extension there is not supported.
The worker reports its detected host-browser identity during the handshake.
Desktop rejects an identity that differs from the selected browser. One
browser/profile is active at a time.
Switching browsers revokes pending work, temporary approvals and the old native
credential, and removes the previous registration. Permission rules remain saved
and apply to the newly selected browser. Existing Chrome-only policies preserve
their Chrome selection; new installations require an explicit choice.

After repairing or switching Desktop installations, click the LingXi Browser
toolbar icon in the selected browser to reconnect its native host, then retry in Desktop.

The extension needs the browser's debugger, tabs, history and download capabilities.
LingXi additionally enforces its own configured approvals for browsing, history
and downloads. The browser may display its debugger indicator. Enterprise restrictions
and the browser's native permission checks still apply.

Each LingXi session owns its own tab group. Existing tabs in other groups are
not exposed. HTTP and HTTPS origins match exactly, including scheme and port;
subdomains need their own setting. Navigation redirects to an unapproved origin
are blocked and require a new explicit request for that origin.

Downloads currently require a previously approved, same-origin session tab,
are limited to 8 MiB, and reject redirects. LingXi fetches the approved content
without following redirects, then asks the browser to save a materialized data URL.
Download permissions apply to downloads explicitly requested through LingXi's
download tool. Target-level interception also blocks identifiable attachment
responses and opaque download requests, but cannot provide browser-wide control
over a website's autonomous blob/data downloads.
Raw CDP access is target-scoped and cannot disable permission guards, create
arbitrary targets, read profile-wide cookies, or change download restrictions.
WebMCP requires the browser's `document.modelContext` API and a same-origin registered
tool on the target page; unavailable APIs produce an explicit error.

Removing the connection in LingXi revokes active access and removes its native
host registration. Remove the extension itself from the selected browser's Extensions page when
it is no longer needed. This extension uses LingXi's transport and account
system; it does not impersonate Anthropic's separately shipped extension.

Native-messaging registration paths follow the browser vendors' documentation:
[Chrome and Chromium](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging),
[Microsoft Edge](https://learn.microsoft.com/en-us/microsoft-edge/extensions/developer-guide/native-messaging),
and [Brave's native-host path setup](https://github.com/brave/brave-core/blob/master/app/brave_main_delegate.cc).

## Mobile clients

Android and iOS expose a searchable **Browser** settings page. External webpages
continue to use the system default browser; Local Apps retain their other
opening methods. Opening a page does not grant automation access.

Android Direct discovers installed browsers through Android's web-intent
handlers. Browser choices use the same app permissions as Computer use. They
remain a template until the user starts a control session; the active session's
actual grants determine readiness. Stop the old session before changing browser
permissions. Android Play and the current iOS client disclose unavailable native
browser control instead of offering a Desktop extension connection.
