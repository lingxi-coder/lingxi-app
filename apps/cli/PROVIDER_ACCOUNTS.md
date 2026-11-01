# Provider accounts

Provider accounts belong to the execution host and its credential storage domain.
The CLI uses the shared Rust account directory, credential store and authorization
coordinator. Account IDs are stable; display names and user priority can change.

```sh
lingxi-cli provider accounts list openai --json
# Pipe one API key from a trusted secret source; no key is accepted in argv.
secret-source | lingxi-cli provider accounts add openai --name 'Work API' --api-key-stdin --json
lingxi-cli provider accounts login openai --name 'Work subscription'
lingxi-cli provider accounts reconnect openai ACCOUNT_ID
lingxi-cli provider accounts reorder openai ACCOUNT_B ACCOUNT_A
lingxi-cli provider accounts disable openai ACCOUNT_A
lingxi-cli provider accounts enable openai ACCOUNT_A
lingxi-cli provider accounts test openai ACCOUNT_A --json
lingxi-cli provider accounts logout openai ACCOUNT_A
lingxi-cli provider accounts remove openai ACCOUNT_A
```

`secret-source` represents an existing trusted command that writes a key directly
to the pipe. API key input from an interactive terminal is rejected to avoid echo.
`--api-key-secret-ref acct.UUID` reads an existing API key slot from the same host
storage domain instead of stdin. OAuth slots cannot be used as API key input.

For a remote shell, use `login` or `reconnect` with `--no-browser --callback-stdin`.
Open the printed authorization URL on your local machine, then pipe the complete
callback URL back to the waiting command. Callback input from an interactive
terminal is rejected to avoid echo; the callback is consumed only by its original
flow and must match the original state and redirect destination.

Use `--execution-binding ID` when a provider needs an explicit catalog binding.
OpenAI subscription login uses `openai-chatgpt`; OpenAI API keys use `openai`.
The shared host capability policy controls which OAuth entries are open.

`--json` can follow any account command and emits one JSON response. Success
contains `ok` and a secret-free `snapshot` with account identity, persistent user
priority, configuration revision and per-account health reasons/recovery times.
Failure contains `ok: false` and a stable error code. Credential values and OAuth
callback URLs are never included. Authorization instructions go to stderr during
interactive login, keeping stdout usable by scripts.

For a script that reads then modifies configuration, pass the snapshot's
`configRevision` using `--expected-revision N`. A competing configuration update
causes a revision conflict instead of overwriting it. Reordering must specify
every account ID for that provider exactly once. Health changes do not rewrite
user priority.

`logout` clears only the selected account's material and retains its account row;
`remove` removes only the selected account. Provider authentication is managed only through `provider accounts`; the old
`auth` and `setup-token` command families have been removed.

The CLI and interactive `/connect` picker create named accounts through the same authority. Environment API keys, `apiKeyHelper`, and the old `auth` / `setup-token` commands are no longer credential inputs.

List and test JSON include the shared `modelEntitlements` and `modelAvailability` projections. `confirmed` records observed inference success, `forbidden` records a model-specific rejection, and `unverified` means inference authorization has not been observed. Model discovery by itself never confirms inference authorization.

For example, if account A is denied model M1, A can remain eligible for M2 while account B serves M1. After the denial expires, a real request can probe A again; a successful probe restores its original priority. Listing or testing model discovery alone does not authorize that recovery.

`--file` accepts JSON `{"resource": <SDK ProviderFileRef>, "relativePath": "docs/input.txt"}` for each startup download. Pass the complete reference returned by the SDK, including its managed `accountScope`, exact account profile, protocol and endpoint fingerprint. The host rejects a removed/replaced account or changed endpoint before loading its credential; it never retries the resource through another account. Bare `file_id:path` inputs and environment API keys are rejected. Downloads are limited to 64 MiB in total and installed only after the entire batch succeeds; existing files are preserved.
