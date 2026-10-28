# Offline Mermaid rendering

Desktop, Android and iOS render completed `mermaid` Markdown fences using the
same bundled viewer. Unfinished streaming fences remain source code. The viewer
supports zoom, source inspection, Chinese labels, line breaks and light/dark
themes. Invalid or oversized diagrams show their original source. Native copy
buttons always copy the source.

`resources/mermaid/mermaid.html` is a generated, self-contained document. It
requires no CDN, network, engine service or host tool bridge. Desktop loads it in
an iframe with only `allow-scripts`; mobile uses a dedicated transcript web view.
This does not affect the external-browser entry point for Mobile Local Apps.

The viewer uses Mermaid strict mode, SVG text labels and limits of 50,000 source
characters / 500 edges. Diagram configuration cannot override host security,
HTML, CSS or theme settings. Scripts are hash-authorized by CSP, network access
is denied, and diagram links/images are removed. Only bounded height messages
leave the viewer.

To update the resource:

```sh
cd packages/mermaid-renderer
npm ci --ignore-scripts
npm run build
npm run check
```

Commit the lockfile, generated HTML and third-party notices together. The sample
in `fixtures/remote-access.mmd` covers the requested remote-access diagram.

Browser checks run in Chromium and WebKit with Playwright. Use an existing
Playwright installation via `PLAYWRIGHT_MODULE_PATH=/absolute/path/to/playwright/index.mjs npm test`.
Optional `MERMAID_CHROMIUM_PATH` / `MERMAID_WEBKIT_PATH` select existing browser
executables. The suite covers the sample, zoom, source, themes, narrow screens,
malformed/oversized input, isolation, rapid updates and Desktop file URLs.
