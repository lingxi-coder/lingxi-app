# Agent artwork

The canonical SVGs remain in Desktop `assets/agent-avatars`. Native clients use
96×96 transparent PNG exports so the exact translucent paths and gradients render
without adding SVG runtime dependencies. Regenerate with:

```
node apps/electron/node_modules/electron/cli.js apps/electron/scripts/export-native-agent-avatars.cjs
```

Android selects explicit light/dark drawable resources from the resolved palette.
iOS uses paired asset catalog appearances and the resolved theme. Both hash UTF-16
code units using base 31 modulo 2147483647 and then 28, exactly as Desktop does.
Shared reference vectors: `packages/bridge-client/fixtures/agent-avatar-identities.json`.
Keep the variant order stable; changing it changes existing agent identities.
