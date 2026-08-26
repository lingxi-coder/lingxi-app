# Claude Code 2.1.245 → 2.1.246 delta audit — 2026-08-25

## Scope

This note only covers the observable delta between the locally installed Claude Code binaries:

- `/Users/luolingfeng/.local/share/claude/versions/2.1.245`
- `/Users/luolingfeng/.local/share/claude/versions/2.1.246`

The official GitHub Releases page currently lists 2.1.245 as latest; no public 2.1.246 release note was visible at the time of this audit:

- <https://github.com/anthropics/claude-code/releases>

## Evidence

| Probe | 2.1.245 | 2.1.246 | Result |
| --- | --- | --- | --- |
| `--version` | `2.1.245 (Claude Code)` | `2.1.246 (Claude Code)` | version bump only |
| normalized `--help` | SHA-256 `71ad650f59e08ae40ede14c534db4f49d8590ee5a4f92f6da2882d3a5560fea6` | same | byte-identical help surface |
| help byte length | `16890` | `16890` | identical |
| help diff | empty | empty | no new top-level CLI flags/commands proven |
| binary type | Mach-O arm64 | Mach-O arm64 | same platform class |

Additional binary metadata:

- `sha256sum` 2.1.245: `9f7c2260251765a18d0b35198669dacc1912f6e8129a3b01f6b58d93365ff1f1`
- `sha256sum` 2.1.246: `7b09f01cb76a38e0e3a7c47c5d698d382162a5ff26538fc778683770caf9218b`
- file size 2.1.245: `376109392`
- file size 2.1.246: `230824016`

## Findings

### Confirmed

- The only user-visible difference proven by direct CLI probe is the version string itself.
- The public `--help` surface is byte-for-byte identical after ANSI stripping.
- I did not prove any new top-level flag, command, or documented prompt/schema surface in 2.1.246.

### Unconfirmed / noisy string deltas

The raw binary string inventory differs substantially, but the differences are dominated by bundled runtime and generated code strings. That signal is not strong enough to claim a user-facing behavior change on its own.

### Working conclusion

Treat 2.1.246 as a packaging or rebuild hotfix unless a later oracle or changelog proves a behavior delta. At the time of this audit, there is no evidence of a new CLI surface or a prompt/schema change that would require a port-side implementation update beyond the version bump.

## Related port update

The live parity constant has been raised to `traits::CLAUDE_CODE_VERSION = "2.1.246"` and the live version-facing test moved to `test-harness/tests/parity_claude_2_1_246.rs`.

## Verification commands

```bash
/Users/luolingfeng/.local/share/claude/versions/2.1.245 --version
/Users/luolingfeng/.local/share/claude/versions/2.1.246 --version
/Users/luolingfeng/.local/share/claude/versions/2.1.245 --help | perl -0pe 's/\e\[[0-9;]*[A-Za-z]//g' > /tmp/claude-2.1.245.help.txt
/Users/luolingfeng/.local/share/claude/versions/2.1.246 --help | perl -0pe 's/\e\[[0-9;]*[A-Za-z]//g' > /tmp/claude-2.1.246.help.txt
diff -u /tmp/claude-2.1.245.help.txt /tmp/claude-2.1.246.help.txt
sha256sum /Users/luolingfeng/.local/share/claude/versions/2.1.245 /Users/luolingfeng/.local/share/claude/versions/2.1.246
```
