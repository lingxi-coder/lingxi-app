# `tengu_refusal_fallback_notice_collapsed` — what it actually needs, 2026-07-26

I went to implement this and stopped before shipping the queue, because the
queue has no producer in this port and cannot get one without a different
subsystem. Verified against `main` @ `91903a009`, at the behaviour sites.

## What the oracle does

`j0m()` (2.1.220 @246308542) is a one-slot queue in front of the
`model_refusal_fallback` system frame. Read verbatim:

```js
function j0m() {
  let held, retracted = new Set, suppressed = 0;
  return {
    accept(b) {
      let out = [];
      if (held !== undefined) {
        if (b.retractedMessageUuids?.includes(held.uuid)) {
          // the incoming banner RETRACTS the held one → collapse it
          retracted.add(held.uuid); suppressed += 1; held = undefined;
        } else out.push(...flushHeld("episode_boundary"));
      }
      if (b.provisional) held = b;                       // ← the crux
      else out.push(emit(b, suppressed > 0 ? "supersedes" : "immediate"));
      return out;
    },
    dropHeld(uuid) { … suppressed += 1 … },
    settle(via) { return flushHeld(via); },
  };
}
```

and the consumer fires the telemetry only when something was actually
collapsed: `if (suppressedCount > 0) M("tengu_refusal_fallback_notice_collapsed",
{suppressed_count, emitted_via})`.

So the event exists to report banners that were shown-then-withdrawn. Every
path to a non-zero `suppressedCount` runs through `b.provisional === true`.

## Why this port can never produce a provisional banner today

Not "does not yet" — **cannot**, and the reason is structural rather than a
missing field.

The oracle marks a refusal banner provisional because it decides refusal while
the stream is still open, so a later tombstone or retraction signal can withdraw
the refused message (`tengu_partial_stream_retraction_closed`, and the six
`tengu_refusal_retraction_*` events beside it).

This port decides refusal at RESPONSE TERMINAL:
`conversation.rs:8275` matches `Some("refusal")` on the completed response's
`stop_reason`, in the same arm-set as `max_tokens` / `stop_sequence` /
`pause_turn`. A refusal that has already been decided cannot be retracted by a
later part of the same stream, because there is no later part.

Corroborating: repo-wide there is no `retract` / `tombstone` / `provisional`
substrate in `orchestrator` or `llm-client` (the single `tombstone` hit is a
comment saying the port deliberately does NOT tombstone a leaked block), and
the port's banner is a plain `output.emit_text(&warning)` — no uuid, so nothing
a `retractedMessageUuids` list could even name.

## Why I did not ship the queue anyway

It would be inert by construction: with `provisional` always false the queue
degenerates to pass-through, `suppressedCount` is always 0, and the telemetry
never fires. That is the dead-code-that-reads-as-coverage pattern this session
has spent several waves undoing — and the next audit would read the queue's
presence as the item being done.

## The real decomposition

1. **Move refusal detection into the stream.** Today it is a terminal
   `stop_reason` match. The oracle evaluates it on partial stream state, which
   is what creates the "might still be withdrawn" window.
2. **Give the banner an identity.** A typed `RefusalBanner { uuid, trigger,
   direction, original_model, fallback_model, request_id, api_refusal_category,
   api_refusal_explanation, refused_user_message_uuid, retracted_message_uuids,
   provisional, content }` — the frame shape at @246323682 — instead of a
   formatted string.
3. **The retraction signals.** `tengu_refusal_retraction_{evicted,late_drop,
   history_dropped,orphan_tool_result,truncation_harvest,
   unauthenticated_signal}` plus `tengu_partial_stream_retraction_closed`. These
   are what populate `retractedMessageUuids`.
4. **Then** `j0m` — which is ~40 lines and the easy part.

Steps 1–3 are one subsystem and must land together; step 4 alone is inert, and
step 4 with only step 2 is still inert.

## Also worth noting

The port's refusal fallback is latched once per session, so a SECOND refusal
takes the terminal arm and surfaces the byte-locked `API Error: …` message
rather than a second banner. That is faithful to the oracle's latch and is NOT
what `suppressedCount` counts — the oracle's collapse is about a banner
withdrawn within one episode, not about the once-per-session cap. Conflating
the two would produce a plausible-looking `suppressed_count` that means
something else.
