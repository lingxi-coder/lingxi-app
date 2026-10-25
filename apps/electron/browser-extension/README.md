# LingXi Browser

This extension connects Google Chrome to LingXi Desktop's device permission
settings. The browser profile stays in Chrome. Model credentials and approval
decisions stay in LingXi Desktop.

For this development distribution:

1. In LingXi Desktop, open Settings → Computer use → Google Chrome and choose
   **Set up extension**. This registers LingXi's native messaging host for the
   current macOS user.
2. Open `chrome://extensions`, enable Developer mode, choose **Load unpacked**,
   and select this directory. The directory must remain in place.
3. Return to LingXi and choose **Retry**. Connected means the extension has
   completed the versioned native handshake; registering the native host alone
   does not mean Chrome loaded the extension.

After repairing or switching Desktop installations, click the LingXi Browser
toolbar icon in Chrome to reconnect its native host, then retry in Desktop.

The extension needs Chrome's debugger, tabs, history and download capabilities.
LingXi additionally enforces its own configured approvals for browsing, history
and downloads. Chrome may display its debugger indicator. Enterprise restrictions
and Chrome's native permission checks still apply.

Each LingXi session owns its own tab group. Existing tabs in other groups are
not exposed. HTTP and HTTPS origins match exactly, including scheme and port;
subdomains need their own setting. Navigation redirects to an unapproved origin
are blocked and require a new explicit request for that origin.

Downloads currently require a previously approved, same-origin session tab,
are limited to 8 MiB, and reject redirects. LingXi fetches the approved content
without following redirects, then asks Chrome to save a materialized data URL.
Download permissions apply to downloads explicitly requested through LingXi's
download tool. Target-level interception also blocks identifiable attachment
responses and opaque download requests, but cannot provide browser-wide control
over a website's autonomous blob/data downloads.
Raw CDP access is target-scoped and cannot disable permission guards, create
arbitrary targets, read profile-wide cookies, or change download restrictions.
WebMCP requires Chrome's `document.modelContext` API and a same-origin registered
tool on the target page; unavailable APIs produce an explicit error.

Removing the connection in LingXi revokes active access and removes its native
host registration. Remove the extension itself from `chrome://extensions` when
it is no longer needed. This extension uses LingXi's transport and account
system; it does not impersonate Anthropic's separately shipped extension.
