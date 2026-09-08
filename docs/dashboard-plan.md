# Dashboard: the path from mock data to the real simulator

Written 2026-09-06 16:10 PDT at Issao's request, to scrutinise why the first dashboard showing real
runs is estimated at 19:00 and not sooner. Owner of the estimate: the tech lead. Owner of this file: the
main agent, until the tech lead's plan supersedes it. **Read as history:** the path below was walked,
and the stand-in's role ended on 2026-09-07 (U100, dcc226d) when the browser mock engine was deleted;
the dashboard's data modes are live and replay, and a field the engine does not produce renders "—".

## 1. What is between the engine and the panels today

```
 crates/sim-leaf         RunResult: per-request records (arrival, admit, first token, finish,
     (engine)            outcome, tokens), fleet Series every sample_interval_ms (250 ms),
        |                replica_load per replica, four whole-run histograms
        |  (A) sim-run export  -> WIRE.md JSON: runs/<id>/status.json, fleet.jsonl, result.json, index.json
        v
 site/runs/<id>/         static files, served by the existing sim-ingress static server, no new server code
        |
        |  (B) web transport client  api.ts / useServerRun.ts, typed to WIRE.md, SSE + unary
        |  (C) replay source         reads runs/<id>/*.json when no Ingress answers, feeds (D)
        v
 web/src/lib/useRun.ts   RunHandle { engine, cursorS, paused, speed, step, rewindTo, scrubTo, update, ... }
        |
        |  (D) adapter              SubscriptionUpdate rows  ->  Frame { offeredRps, ttft: Histogram,
        v                                                         replicas: ReplicaSample[], ... }
 web/src/panels/*        every panel reads Frame fields; 3,500 lines written against the MockEngine's Frame
```

The panels never see the wire. They read `Frame` (`web/src/lib/engine.ts:66`), the mock engine's
per-tick record, through `RunHandle`. So "linking the dashboard to the real simulator" is exactly four
units: (A) get real runs into the wire format, (B) speak the wire from the browser, (C) load pre-baked
runs when no server is present, (D) turn wire rows into `Frame`s. Nothing else changes.

## 2. Why the estimate is 19:00

| Unit | Agent | State at 16:10 | Estimate | Depends on |
|---|---|---|---|---|
| (A) `wire.rs` + `export.rs` + `sim-run export` | export agent, `claude/tl-export` | started 16:04 | 1.0 h → 17:05 | nothing |
| (B) transport client rebased onto WIRE.md | web agent, `claude/tl-web` | running since 15:46 | ~0.5 h left → 16:45 | nothing |
| (C)+(D) replay source and Frame adapter | not started | queued **behind (B)** | 1.5 h → 18:15 | (B) done, because both touch `web/src/lib/types.ts` and `useRun.ts` |
| integrate, fingerprints, build, deploy | tech lead, then main agent | | 0.5 h → 18:45 | (A), (C), (D) |

Rounded: 19:00. The critical path is (B) → (C)+(D) → integrate. (A) is off the critical path; it
finishes an hour before it is needed.

**The hour that can be taken back.** (C)+(D) wait for (B) for a file-ownership reason, not a technical
one: the adapter needs the TypeScript types for `SubscriptionUpdate`, and the transport agent owns
`types.ts`. But WIRE.md fixes those shapes in prose, so the adapter can be written now against WIRE.md
with its own local types and reconciled at integration, a fifteen-minute merge. Starting (C)+(D) now
puts them at ~17:45 and the deploy at ~18:00. I have asked the tech lead to do this (section 6).

## 3. What will be real and what stays mock in the first linked version

The engine records what it simulates, and the mock invents things the engine does not simulate yet.
Panels are wired field by field, and every field the engine cannot supply keeps its "mock" marker.

Real, from `RunResult` today:

| Frame field | Engine source | Panel |
|---|---|---|
| `offeredRps` | `Series offered_rps` | load, status bar |
| `completedRps`, `rejectedRps`, `admittedRps` (= completed until an admission series exists) | records bucketed by finish time into the 250 ms window | load |
| `outputTokensPerS` and goodput | tokens of records finished in the window | throughput |
| `loadImbalanceCv` | `replica_load` at the sample | imbalance |
| `kvUtilization` | `fleet_kv_utilization` | KV |
| `ttft`, `itl`, `e2e`, `queueWait` histograms | records finished in the window, p50/90/99/99.9 | latency, all four |
| `readyReplicas` | replica count | fleet |
| `replicas[].queuedSeqs`, `runningSeqs`, `kvUtilization` | per-replica frames, landing in the engine-core unit (`claude/tl-engine`) | heatmap |

Not simulated yet, so the panel keeps its mock marker or shows a dash:

| Frame field | Why absent | Arrives with |
|---|---|---|
| `prefixHitRate`, per replica and fleet | no prefix cache in the engine | prefix caching unit (scope-today items 9, 10) |
| `preemptionsPerS`, `wastedGpuFraction` | no preemption in the engine | KV capacity and preemption unit (items 7, 8) |
| `tierUtilization`, `tierBandwidth` | no memory tiers | tiering unit (item 14) |
| `warmingReplicas`, `drainingReplicas`, `ejectedReplicas`, `replicas[].state` | no replica lifecycle or failure injection | autoscaling and failure units (items 12, 4b) |
| `replicas[].trueSpeedMultiplier` | no gray failure | failure unit |
| `replicas[].telemetryStalenessMs` | the engine delays telemetry but does not record staleness per replica | small engine addition, after frames |
| `replicas[].prefixHitRate`, `preemptionsPerS` | as above | as above |

So the first linked dashboard shows the load, throughput, latency, imbalance and KV panels with real
numbers from the six demo runs, and the heatmap once per-replica frames land. The tiering, prefix and
failure panels stay visibly mock until the corresponding dynamic exists in the engine, which is the
honest state.

## 4. What the controls mean on a pre-baked run

A pre-baked run is complete: every sample from start to end is in `fleet.jsonl`. So:

- **Play, pause, speed, step, scrub**: work entirely in the browser, since the whole run is loaded.
  Scrubbing never re-simulates, which is the M8 requirement met trivially.
- **Rewind and re-simulate from a snapshot, change workload or policy mid-run**: need a live engine.
  In replay mode these controls are disabled with a tooltip saying why, rather than pretending. They
  return with the live Ingress endpoint (section 5).
- **A/B compare**: two pre-baked runs side by side, both real. The six demos already come in pairs.

## 5. After the pre-baked version: the live path

| Step | Estimate | Gate |
|---|---|---|
| Engine resumable: `Sim` advances to a time and back from a snapshot, per-replica frames | in progress, `claude/tl-engine` | byte-identical fingerprints |
| Ingress `StartRun`, `GetRun`, `OpenSubscription` (SSE with `Last-Event-ID` replay), `SetSpeed`, `StepForward`, `Rewind` | ~2 h after resumable | `cargo test -p sim-ingress` field-name test |
| Web on the live transport; mock markers removed only on wired panels | ~1 h after | `npm run build` |
| Rebuild, redeploy, idle-to-zero re-measured with a subscription open then closed | 0.5 h | `./deploy.sh --check-idle` |

The tech lead's estimate for this is 21:00 for the endpoint and 22:30 deployed. The pre-baked encoder
(A) and adapter (D) are reused unchanged: the live stream carries the same documents `fleet.jsonl`
holds, one per SSE event.

## 6. Decisions asked of the tech lead, 16:10

1. Start (C)+(D) now against WIRE.md, not after (B). Expected saving: one hour.
2. Ship fleet-only first; the heatmap follows when per-replica frames land, as a second deploy.
3. Replay mode disables rewind and update with a visible reason; it does not hide them.

## 7. Risks worth knowing about

- **Window histograms at 250 ms.** At 70 rps a window holds ~17 completions, so p99 of a window is
  noisy by construction. The panels smooth over a longer window client-side (`mergeWindow` in
  `engine.ts`), which is why the export carries windows, not cumulative histograms.
- **File size.** A 600 s demo at 250 ms is 2,400 fleet rows; with four distributions per row, about
  2 MB per run as JSON lines, six runs ~12 MB in the image. Acceptable; gzip is on by default on Cloud Run.
- **Field-name drift.** No generated types on either side. The export crate carries a test that parses
  `subscription.proto` and checks every metric name and number it emits; the transport client should
  carry the mirror test. That is the only thing keeping the two sides honest until `prost` lands.
- **The Frame shape is the mock's, not the proto's.** The adapter (D) is the place where that debt
  lives, deliberately, so that panels do not have to be rewritten twice. When the live path lands, the
  question of whether panels should read wire rows directly is worth revisiting.
