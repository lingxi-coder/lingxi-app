# LingXi Desktop Mods 0.1.1

Maintained, Apache-2.0 distributions of Anthropic's Replay Theater and Token
Weather. Their executable source matches the modules accepted in the signed
Desktop GUI, plus one maintenance-notice comment. `provenance.json` records the
upstream revision and accepted module hashes; tests verify this relationship.

## Install from this repository

Use an absolute path to this directory in a durable checkout:

```sh
lingxi-cli plugin marketplace add /absolute/path/to/lingxi-app/plugins/desktop-mods
lingxi-cli plugin disable replay-theater@claude-code-playground-mods --scope user
lingxi-cli plugin disable token-weather@claude-code-playground-mods --scope user
lingxi-cli plugin install lingxi-replay-theater@lingxi-desktop-mods --scope user
lingxi-cli plugin install lingxi-token-weather@lingxi-desktop-mods --scope user
```

The same operations are available in Desktop Settings > Plugins. Disable the
upstream versions before enabling these distributions to avoid duplicate edit
recording and duplicate bands. Distinct `lingxi-` manifest names prevent
same-name discovery/catalog collisions with retained upstream installations;
this is a maintained fork, not an alias or a compatibility layer.

Updates and reinstalls now read the checked-in 0.1.1 modules instead of an
unmodified upstream directory. Advance both plugin and marketplace versions for
future releases. Do not edit installed cache files as the maintained source.
A directory marketplace depends on its source checkout: keep that directory
available. This is a local, version-controlled distribution, not a newly
published public marketplace or an automatically bundled Desktop component.

## Behavior and tests

- Weather displays current session context usage and a bounded reading history.
- Both AbovePrompt middleware handlers retain downstream UI and compose their
  own content; neither embeds the empty Host engine marker in a child tree.
- Empty initial draws remain eligible for invalidation.
- Replay falls back to AbovePrompt when the Pane API is unavailable.
- Close hides the expanded replay, keeps its reopen hint and Weather, and
  retains the last editing turn. A read-only turn does not erase that replay.

```sh
cd plugins/desktop-mods
npm test
```

Five dependency-free Node tests cover both middleware orders, open/paging/close,
read-only turn retention, initially empty usage, current usage changes, manifest
paths/versions, release hashes and equivalence to the GUI-accepted module bodies.
The signed production Bridge also passed install/uninstall/reinstall in an
isolated home, with exact installed source hashes checked.

See LICENSE and NOTICE, also included in each installed plugin directory.
