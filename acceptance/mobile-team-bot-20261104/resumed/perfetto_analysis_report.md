# Android scroll acceptance: short debug run

Question: can the production Teams UI scroll a synthetic 500-conversation
directory and a 400-message window, and does an always-composed hidden Chats
transcript add visible frame-time cost? Source: `194234165`, Samsung SM-S9310,
Android 16/API 36 over USB, final matching debug APKs.

The fixture uses production list/conversation/shell composables and a named,
isolated Firebase SDK instance pointed at loopback. It never starts the workspace
or signs in. All messages and members are synthetic; no cloud resource, production
account or live endpoint is used. It uses real Choreographer/Window FrameMetrics,
not Compose's virtual test clock. Six real swipes follow each settle period.

## FrameMetrics results

| Phase | Frames | P50 ms | P95 ms | Over deadline | PSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| Directory, light Chats A | 341 | 5.57 | 11.37 | 11 | 256.5 |
| Directory, hidden 400-message Chats A | 342 | 5.83 | 11.34 | 12 | 268.8 |
| Directory, hidden 400-message Chats B | 343 | 6.41 | 11.38 | 9 | 250.4 |
| Directory, light Chats B | 344 | 5.97 | 11.25 | 11 | 255.5 |
| Teams 400-message window | 349 | 5.84 | 9.16 | 8 | 265.7 |

Callback drops were zero in every phase. Mean directory P95 was **11.3085 ms** for
the light control versus **11.3578 ms** with hidden Chats: **+0.0493 ms** in this
ABBA sample. No material P95 increase was observed here. Earlier short runs varied;
this is not a statistical equivalence proof. The dedicated layout regression also
verifies that hidden Chats remains composed without measuring its TopAppBar, both
before first display and after visible → hidden updates.

There are still missed deadlines. PSS samples span 250.4–268.8 MiB; they do not
establish leak freedom. The dataset is text-only, offline and short-lived. No
release-build, network, media-heavy, streaming, thermal-soak or field acceptance
is established by these figures.

## Perfetto corroboration

Trace (ignored local artifact):
`output/mobile-team-bot-20261104/samsung-scroll-final-confirmed.perfetto-trace`.
Its SHA-256 and size are in `validation.json`. The committed
`perfetto-config.pftxt` streams buffers to disk, retaining all five named phases
and the app's process metadata. Earlier ring-buffer attempts are diagnostic only.

The skill-bundled processor is v58.3. Full sorting removed the initial 20
out-of-order-event diagnostics without changing the phase statistics. Final
processing reported no positive error/data-loss-severity counters. An informational
`traced_chunks_discarded=5` remains; lossless producer capture is not claimed.

| Trace phase | android_frames | Actual FrameTimeline frames | App-jank classified frames |
| --- | ---: | ---: | ---: |
| directory-light-a | 349 | 343 | 10 |
| directory-heavy-a | 352 | 344 | 12 |
| directory-heavy-b | 349 | 345 | 10 |
| directory-light-b | 350 | 347 | 10 |
| messages-400 | 354 | 351 | 8 |

Trace frame-start membership and asynchronous FrameMetrics callback windows are
different measurements, so their frame counts need not match exactly. The CSV
duration column uses `android_frames.dur` (with its documented fallback for frames
without an actual timeline); it is not the FrameMetrics P95 render-cost metric.

## Validated SQL and reproduction

Load this trace once with the skill-bundled wrapper, using `server unix --name
mobile-scroll --daemonize --full-sort TRACE_PATH`; execute queries with `query
--remote mobile-scroll`, then `server kill mobile-scroll`. The wrapper used here is
`/Users/luolingfeng/.codex/skills/android-profiler/references/perfetto/bin/trace_processor`.
Schema and standard-library documentation were checked before these queries.
Only public interfaces appear below. Frames are assigned by their start timestamp
inside each non-overlapping named test phase; no CPU overlap duration is inferred.

```sql
INCLUDE PERFETTO MODULE android.frames.timeline;
INCLUDE PERFETTO MODULE android.frames.jank_type;
SELECT s.name AS phase,
       COUNT(*) AS frames,
       ROUND(AVG(f.dur) / 1000000.0, 3) AS avgPresentationLatencyMs,
       COUNT(a.id) AS frameTimelineFrames,
       SUM(COALESCE(android_is_app_jank_type(a.jank_type), 0)) AS appJankFrames
FROM slice AS s
JOIN android_frames AS f ON f.ts >= s.ts AND f.ts < s.ts + s.dur
LEFT JOIN actual_frame_timeline_slice AS a ON a.id = f.actual_frame_timeline_id
WHERE s.name GLOB 'mobile-port/*' AND s.dur > 0 AND f.dur > 0
  AND f.process_name = 'com.lingxi.code.debug'
GROUP BY s.name
ORDER BY s.ts;

SELECT s.name, s.value, s.severity
FROM stats AS s
WHERE s.value > 0
  AND (s.severity IN ('data_loss', 'error')
       OR s.name GLOB '*overwrit*' OR s.name GLOB '*discard*');
```

Raw fully sorted query output is in `perfetto-results.csv`; original callback
measurements are in `scroll-performance.json`. A longer repeated release-build
comparison with real accounts and workloads remains necessary for broad performance
acceptance.
