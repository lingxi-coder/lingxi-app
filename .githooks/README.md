# Local git hooks

One-time setup, per clone:

```
git config core.hooksPath .githooks
```

That points this clone's git at `pre-commit`, which runs
`lingxi-code/scripts/check-all.sh` (every gate script in `lingxi-code/scripts/`,
including Phase 2 Plugin migration checks, plus the LAP task descriptors)
and its trigger regression test before a commit is created. It is opt-in (git never auto-adopts a
`hooksPath`) and it is bypassable (`git commit --no-verify`) — this is a
speed bump for the machine where the work happens, not an enforcement
boundary. CI independently runs the same chain for every push to `main` and
pull request.
