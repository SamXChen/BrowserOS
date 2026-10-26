Capture HTTP traffic from the user's browser and install mock rules.

Typical loop: the user clicks Copy reference on a request and pastes the `netref_*` id. Call `handoff_get` to read the request/response, then `rule_apply` with a short `name`. Rules have names: later pass the name (or `rule_*` id) as `rule` to `rule_get`, `rule_apply`, `rule_status` or `cleanup`; `rule_list` lists them. Never invent a sample from a URL: a missing or expired reference means asking the user to copy it again.

## Applying

- `rule_apply` answers `active` only once the browser acknowledged Fetch interception; `pending` means saved and waiting for the browser, and it goes live on its own.
- `dryRun: true` reports the matched samples, the winner and the rules it would replace.
- Prefer `response.body` `{kind:"patch_sample"|"patch_current", jsonPatch:[…]}` for small edits; `literal` only for a whole intentional body. Large bodies are paged with `bodyOffset`/`bodyLimit`; do not paste them back.
- For edits that depend on live upstream data, pass `processor: {script}`: a synchronous `function process({request, response})` returning `{status?, statusText?, headers?, setHeaders?, body?}` or `undefined`. It may rewrite any header, `set-cookie` included; failures pass the original through.

## Matching

- Keep `matcherSeed`; it is what lets a later sample hit the same rule.
- `ignoreKeys` only for names like nonce/ts, or values that differ across related samples. `strictQuery` or `bodyFields` when the user must tell two operations of one POST endpoint apart. Both replace the previous lists.

## Scope and lifecycle

- Rules apply to every tab by default (`scope:"all_tabs"`); say so.
- "Turn it off" is `rule_apply` with `enabled:false`, not deletion. Delete only when asked; batch `cleanup` (`target:"rules"|"records"|"handoffs"`) must first report the count and wait for confirmation. Deleting a rule makes the next request real.
- Do not refresh the page unless the user asks.

## Reading state

`rule_get` and `rule_status` answer `state` plus a `reason` when not active.

- States: `active` (acknowledged), `pending` (saved, waiting for the browser, goes live automatically), `unbound` (its tab is gone), `failed` (the stored rule cannot be used), `disabled` (intent off, or it lost a replacement or a stop-all).
- Reasons: `no_browser`, `no_tab`, `apply_timeout`, `tab_closed`, `body_missing`, `body_too_large`, `processor_invalid`, `stopped_all`, `superseded` (replaced, or the user stopped all mocks), `replaced_by:<id>` (the rule that took over), `user` (switched off by the user).
- `rule_status.hits` outcomes are strings: `fulfilled`/`processed` served the mock, `blocked` is usually CORS, `failed`/`processor_error`/`processor_timeout` mean the mock or script failed, `passthrough` let the real request through deliberately, `decision_timeout`/`delay_cancelled` are timing, `unknown` was not recorded.
- `nearMisses` explains a miss: `no_request_seen` when the request never fired, or query/method/body differences.
