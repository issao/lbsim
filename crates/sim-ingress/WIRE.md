# The Frontend-to-Ingress wire, as served today

`proto/lbsim/v1/ingress.proto` and `subscription.proto` are the contract. This file is the mapping of
that contract onto what the server actually speaks, so the web client and the server are built against
one document by two agents at once.

## Decision: JSON over HTTP/1.1, server-sent events for subscriptions, no gRPC yet

`tonic` would bring roughly a hundred crates, a `protoc` step at build time and `tonic-web` for the
browser, against a project whose inner loop is a one-second zero-dependency build. The existing server in
`crates/sim-ingress/src/lib.rs` already parses HTTP/1.1, Cloud Run streams responses, and every browser
has `fetch` and `EventSource`. So:

- **Unary RPCs** are `POST /v1/ingress/<RpcName>` with a JSON request body and a JSON response body.
  `<RpcName>` is the proto method name verbatim: `StartRun`, `StopRun`, `GetRun`, `ListRuns`,
  `SetSpeed`, `StepForward`, `Rewind`, `UpdateWorkload`, `UpdatePolicies`, `RenewSubscription`,
  `CloseSubscription`, `GetResult`, `GetTraces`.
- **`OpenSubscription`** is `GET /v1/ingress/OpenSubscription?<query>` returning `text/event-stream`.
  The first event is an `OpenSubscriptionResponse`; every later event is a `SubscriptionUpdate`. The
  stream closes when the lease expires, when `CloseSubscription` is called, or after the `final` update.
  One proto RPC, one HTTP request: a browser cannot open a bidirectional stream, so the *renew* and
  *close* halves are the unary calls above. That split is a transport detail of this mapping and is
  not in the proto.
- **Reconnect.** Every SSE event carries `id: <n>`, the subscription's delivered-update sequence number
  starting at 1. A client that reconnects sends `Last-Event-ID: <n>` together with `subscription_id` as a
  query parameter on the same `GET /v1/ingress/OpenSubscription`; the server replays from its
  per-subscription ring of the last 256 updates, or answers **HTTP 410 Gone** when the gap is larger than
  the ring, the subscription has already closed, or the subscription is unknown, and the client
  resubscribes from scratch. The lease is what makes this safe: an unrenewed subscription is gone, ring
  and all.
- **Health** is `GET /health` and `GET /healthz`, plain `ok`, touching no run state.
- Anything else is served as a static file from `--dir`, exactly as today.

The cost, stated: no generated types. Field names are hand-written on both sides, and the only thing
holding them to the proto is the rule below plus a test in `crates/sim-ingress` that every JSON field
name it emits appears in `ingress.proto` or `subscription.proto`. When `prost`/`tonic` land, the
JSON shape here is already proto3's canonical JSON mapping, so the switch is a transport change, not a
schema change.

## Encoding rules

1. **Field names are the proto field names verbatim**, snake_case. `sim_time_unix_ns`, not `simTimeNs`.
2. **Every `uint64` is a decimal string**, never a JSON number. Epoch nanoseconds are ~1.77e18, above
   the 2^53 that JavaScript resolves exactly. This is also what proto3 canonical JSON does.
3. **Enums are their proto names as strings**: `"STATE_RUNNING"`, `"SCOPE_FLEET"`, `"METRIC_TTFT"`.
4. **Maps keyed by an enum** (`MetricRow.values`, `MetricRow.distributions`) use the enum's *number* as
   the string key, because that is the stable identity and the client already indexes by number
   (`web/src/lib/types.ts`). `{"40": 70.0}` is `METRIC_OFFERED_RPS`.
5. Absent optional fields are omitted, not `null`.
6. Errors are HTTP 400 (bad request), 404 (unknown run or subscription), 409 (wrong state), 500, with
   body `{"error": "<message>"}`. A rejected subscription open is HTTP 200 with `rejected_reason` set,
   as the proto specifies.
7. Request bodies are capped at **1 MiB**, rejected before the body is read. The server answers `100
   Continue` to an `Expect: 100-continue` request rather than leaving the client to guess whether to
   send the body.

## The one deviation from the proto: the scenario

The engine's `Scenario` is the flat `key = value` text in `scenarios/*.txt`, not the proto `Scenario`
message. Until proto codegen exists, `StartRunRequest.scenario` carries that text:

```json
{
  "scenario": { "text": "name = p2c\nseed = 20260906\n...", "overrides": { "arrival_rps": "90" } },
  "max_realtime_factor": 0,
  "record_traces": false
}
```

`text` is exactly what `sim-run run` reads; `overrides` are the `--set k=v` pairs. `UpdateWorkload` and
`UpdatePolicies` likewise carry `{"run_id": "...", "overrides": {...}}` restricted to workload keys and
policy keys respectively (`Scenario::override_kind` is the classifier). Both are **forward-only**: the
batch is handed to the run thread and applied through `Sim::apply_overrides` at the next step
boundary, between two chunks of simulated time, whether the run is advancing or paused; the RPC
blocks until that happens. Nothing is rewound, so `required_resimulation` is `false` and
`rewound_to_unix_ns` is `0`. The whole batch is checked before anything moves: a key from the other
group ("routing is a policy key; use UpdatePolicies"), a structural key (`replicas`, `seed`, the
physics), an unknown key, or a value that does not parse rejects the entire batch as 200 with
`accepted: false` and `rejected_reason`, leaving the run exactly as it was. A policy key rebuilds the
policy with empty state (a round-robin cursor returns to zero); a workload key takes effect at the
next arrival. A run that is complete or failed answers 409.

## Messages, as JSON

`RunStatus`:
```json
{ "run_id": "r-1", "state": "STATE_RUNNING", "sim_time_unix_ns": "1767225615000000000",
  "sim_end_unix_ns": "1767225720000000000", "realtime_factor": 2.0, "error": "" }
```

`OpenSubscription` query parameters, mirroring `OpenSubscriptionRequest`:
`run_id`, `scope` (`SCOPE_FLEET` | `SCOPE_REPLICA`), `replica_id` (required for `SCOPE_REPLICA`),
`metrics` (comma-separated `METRIC_*` names), `samples_per_sim_second` (float), `percentiles`
(comma-separated floats), `lease_ns` (wall-clock, decimal string), `smoothing_window_ns` (simulated,
decimal string, 0 or absent for the raw cadence; see "Smoothing" below).

First event, `OpenSubscriptionResponse`:
```
event: open
data: {"subscription_id":"s-7","lease_expires_at_wall_ns":"..."}
```

Every later event, `SubscriptionUpdate`:
```
event: update
data: {"subscription_id":"s-7","sim_time_unix_ns":"...","realtime_factor":2.0,
       "row":{"target":{"scope":"SCOPE_FLEET"},
              "values":{"40":70.0,"23":12.0},
              "distributions":{"1":{"count":"41","mean":812.5,"min":120000000.0,"max":2400000000.0,
                                    "percentile":[50,99],"value":[700000000.0,2100000000.0],
                                    "from_merged_histogram":true}}},
       "final":false}
```
Distribution `count` is `uint64` and therefore a string; `mean`, `min`, `max` and `value[]` are
doubles in nanoseconds, as the proto declares them. A distribution at time `t` describes the requests that **completed in the
sample window ending at `t`**, one window per `1 / samples_per_sim_second` simulated seconds, so a chart
of p99 is a chart of the recent tail, not a cumulative one. `count` says how many that was.
`from_merged_histogram` is `true` on every row the live server emits, windowed rows included: the engine
tracks distributions as bucketed Frame histograms and the server reads them as-is. `sim-run export`'s
`fleet.jsonl` computes its windowed rows straight from the exact per-request samples and writes
`from_merged_histogram: false` (see below), so a client must not treat a live row and its later-exported
counterpart as bit-identical merely because both describe the same window.

### Smoothing

`smoothing_window_ns` is `OpenSubscriptionRequest.smoothing_window_ns`: simulated nanoseconds, a
decimal string like every `uint64`, and a malformed value is HTTP 400. It lives on the subscription
rather than in the client, per Issao: *"ideally it is a metric subscription that we pass down to the
leaves"*, so the same window means the same thing on a live stream and on a recording, and a viewer
changing it reopens the subscription rather than re-deriving numbers from samples it may not hold.

Every row is built over the **trailing window ending at its sample**: the recorded frames whose
instant lies in `(t − window, t]`, which at the engine's cadence is `ceil(window / sample_interval)`
frames, never fewer than one. A window of zero, or shorter than one sample, is the raw cadence, and a
row built over one frame is bit-for-bit the raw row. A run's first seconds smooth over the frames that
exist rather than wait for a full window. `sim_time_unix_ns` stays the sample's own instant and the
row count is unchanged, so a smoothed stream lines up with a raw one point for point. The rules, in
`run::row_over`:

- every gauge and rate in `values` is the **mean of its per-frame values** over the window (a frame
  covers one sample interval, so the mean of per-frame rates is the rate over the window);
- a fraction of requests or of time (`METRIC_SLO_ATTAINMENT`, `METRIC_GPU_COMPUTE_BOUND_FRACTION`,
  `METRIC_PREFIX_HIT_RATE`, and per replica `METRIC_GPU_UTILIZATION` and `METRIC_GPU_USEFUL_FRACTION`)
  is the **ratio of the window's sums**, so a frame that ended two requests does not weigh as much as
  one that ended two hundred, and a replica's useful or busy nanoseconds are summed over the window
  before they are divided by it;
- every latency `Distribution` is the **merge of the frames' histograms**: p99 over a 30 s window is
  the p99 of every request that finished in those 30 s, not an average of per-frame p99s (percentiles
  are not mergeable; bucketed histograms are), and `from_merged_histogram` stays `true`;
- the distributions over replicas (`METRIC_GPU_UTILIZATION`, `METRIC_GPU_USEFUL_FRACTION`,
  `METRIC_KV_UTILIZATION`) are taken over each replica's mean across the window, since the question
  is how many replicas sat idle over the window while others saturated;
- `METRIC_READY_REPLICAS`, `METRIC_WARMING_REPLICAS`, `METRIC_DRAINING_REPLICAS` and a replica row's
  `METRIC_REPLICA_STATE` are read **at the sample**, never averaged: a fraction of a replica is not a
  count, and the dashboard enumerates replica ids from the ready count.

The checkpoint (`runs/<run_id>/fleet.jsonl`) and `sim-run export` are always raw; smoothing is a view
of the stream, not of the record.

## What the first server supports

The live server serves `StartRun`, `StopRun`, `GetRun`, `ListRuns`, `SetSpeed`, `StepForward`,
`UpdateWorkload`, `UpdatePolicies`, `RenewSubscription`, `CloseSubscription`, `GetResult`,
`GetTraces`, and `OpenSubscription` today, while `Rewind` answers **HTTP 501 Not Implemented** rather
than 404, so a client can tell "not built yet" apart from "wrong path".

`GetTraces` answers at any state of the run, newest first by the instant the request ended, from the newest 2,000 sampled journeys (or
5 MiB of them encoded, whichever bound trips first) the run thread drains from the engine after every
chunk; it filters by `outcome` (name or number; the proto's zero is "any", and an outcome the engine
never produces, cancelled or failed, gives an empty list), by `min_e2e_ns` against the request's
latency to whatever ended it, and by `tenant_id` (zero is "any"), and returns at most `limit` traces,
100 when unset. `StartRun { record_traces: true }` sets a 5 % sample (`trace_sample_rate = 0.05`)
when the scenario names none, and a scenario with its own rate keeps it; a run that recorded nothing
answers an empty list, not an error.
A span's `operation` is one of the engine's `SpanKind` names: `queue` (gateway or replica, told apart
by `component`), `route`, `prefill`, `prefill_wait`, `decode`, `kv_fetch`, `preempted`. `prefill_wait` is
a step a sequence spent admitted to the batch with prompt left and none of the step's prefill budget
reaching it; its `tokens_processed` is the prefill the step spent on the sequences ahead of it. One
per step waited, so a journey's replica spans are contiguous and a trace carries no unaccounted time.

`preempted` is a step a sequence spent evicted from the batch, waiting to resume: dropped for a
recompute (`kv_tier` `MEMORY_TIER_NONE`) or moved to a lower tier (`MEMORY_TIER_DRAM` or
`MEMORY_TIER_SSD`) for a swap. Its `tokens_processed` is the KV tokens that were dropped or moved,
the sequence's resident total at eviction (prompt plus whatever it had already generated, whatever
its prefill progress, since a running sequence's prompt is charged in full at admission). One per
step it waits, the same granularity as `prefill_wait`, so several steps of waiting read as
contiguous spans; re-admission is an ordinary `Admitted` event (the same one a fresh admission gets),
so the chain into whatever comes next has no gap, and a recomputed sequence's second prefill is a
normal `prefill` span.

Scopes: `SCOPE_FLEET` and `SCOPE_REPLICA`. Everything else returns `rejected_reason`.

| Metric | Fleet | Replica | Source in the engine |
|---|---|---|---|
| `METRIC_OFFERED_RPS` 40 | yes | | `Workload::rate_at` |
| `METRIC_COMPLETED_RPS` 42, `METRIC_ADMITTED_RPS` 41, `METRIC_REJECTED_RPS` 43 | yes | | completions per sample window |
| `METRIC_OUTPUT_TOKENS_PER_S` 44, `METRIC_GOODPUT_TOKENS_PER_S` 45 | yes | | tokens of completions per window, all / within SLO |
| `METRIC_QUEUED_SEQS` 23, `METRIC_RUNNING_SEQS` 22 | sum | yes | replica queue and batch |
| `METRIC_KV_UTILIZATION` 20, `METRIC_KV_TOKENS_RESIDENT` 21 | mean / sum | yes | replica KV |
| `METRIC_STEP_TIME` 8 | | yes | last step duration, seconds as a double like every other duration gauge |
| `METRIC_GPU_UTILIZATION` 67 | mean, and a distribution over replicas | yes | time in step: `busy_ns / window`, what nvidia-smi calls GPU-Util; at batch 1 this is ~1 while 71 is ~1/256 |
| `METRIC_GPU_USEFUL_FRACTION` 71 | mean, and a distribution over replicas | yes | useful work over the maximum possible in the window: `useful_ns / window`, where a decode step is useful for its batch against the effective batch limit and a prefill chunk in full; 71 over 67 is the mean batch fill |
| `METRIC_GPU_COMPUTE_BOUND_FRACTION` 68 | ratio of sums | yes | of busy time, the part priced at the compute roofline |
| `METRIC_LOAD_IMBALANCE_CV` 64 | yes | | CV of per-replica load at the sample |
| `METRIC_SLO_ATTAINMENT` 66 | yes | | window completions within SLO / all window completions |
| `METRIC_TTFT` 1, `METRIC_ITL` 2, `METRIC_E2E` 3, `METRIC_QUEUE_WAIT` 4 | yes | | window histograms |
| `METRIC_READY_REPLICAS` 60 | yes | | replica count, until lifecycle exists |

`METRIC_STEP_TIME` at replica scope is a `value`, seconds as a double like every other duration
gauge and the same on both the live and exported paths — never a `Distribution`, even though it is
one sample rather than a window — and is omitted entirely until that replica has stepped at least
once; a client should read its absence as "no step yet", not as zero.

Sample cadence: the server honours `samples_per_sim_second` as asked. The engine records at the
scenario's `sample_interval_ms`; a subscription rate finer than that gets the nearest recorded sample
repeated, and the response's `sim_time_unix_ns` is the recorded sample's time, never an interpolated
one, because an interpolated batch composition never existed.

## Leases and idle shutdown

`lease_ns` is wall-clock, and so is `lease_expires_at_wall_ns`, which deliberately breaks the
`_unix_ns` convention that everywhere else means a *simulated* instant. A client must not compare it to
the browser's clock, which disagrees with the server's; it counts `lease_ns` down locally and treats the
renew response's `expired` flag as the only authority. Absent, `lease_ns` defaults to 60 s; `lease_ns: 0`
means the subscription is dead on arrival, for a client that only wants a single snapshot. The server
drops a subscription whose lease expired without renewal and ends its stream; a lease also closes on its
own once a `final` update has gone out, since nothing is left that will ever renew it.

A run counts as **idle** — eligible for the shutdown checkpoint below — when it has no live lease and no
queued work. "Queued work" means precisely: an unpaced running run (advancing as fast as the host will
let it) or a run mid-`StepForward` is busy regardless of leases, because it is doing something whether or
not anyone is watching; a paused run, a completed run, and a *paced* running run (bounded by
`realtime_factor`) that has no live lease are all idle, because nothing further will happen to any of
them until a lease or a step arrives. A run with no live lease and no queued work for
`IDLE_SHUTDOWN_SECONDS` (default 300) checkpoints and stops advancing, so a Cloud Run instance can be
reaped; `GetRun` on such a run reports `STATE_PAUSED` with `error` empty.

On Cloud Run the same rule applies sooner, and by a different mechanism: the container's CPU is
request-scoped, throttled to nothing when no request is in flight, so a paced run advances only while
some request is open. An SSE subscription is such a request, which is why the dashboard case works: the
run advances for as long as its viewer is connected. A paced run left with no subscriber stands still,
and `GetRun` shows `sim_time_unix_ns` not moving until someone reconnects; that is the intended idle
behaviour, the paragraph above applied by the platform rather than the guard, not a fault to report. A
fire-and-forget result comes from an unpaced run, `StartRun` without `max_realtime_factor`, which is
busy in its own right and finishes whether or not anyone watches.

The checkpoint is the same four documents `sim-run export` writes under `runs/<run_id>/` —
`status.json`, `scenario.txt`, `fleet.jsonl`, `result.json` — not a resumable engine snapshot:
`Leaf::snapshot` is unimplemented. Reopening a subscription resumes the run from the in-memory `Sim`
that went idle, not from the checkpoint, so the run is only resumable while that process is still up;
the checkpoint exists so a paused run's state can be read, not so it can survive the process restarting.

`StopRun` is a different, deliberate ending: it finalises the run as `STATE_COMPLETE` with every rate
metric computed over the full scenario duration rather than the partial run elapsed so far — the same
convention `sim-run export` uses for `result.json`. These are `lease.rs` and `idle.rs` in this crate.

The server holds three locks around a subscription: the lease registry, the subscription map (each
subscription's ring and cursor), and the run's state. They are taken in that order, **leases → subs →
run.state**, each optional and never reversed: `reap` takes leases then subs, a stream's writer takes subs
then run.state, and the run thread reads its leases before it takes its own state. One writer used to
relock subs while holding it, which on a std mutex parks the thread forever; that one parked thread then
stalled every `reap` and every fresh `OpenSubscription` on the process, which is how lbsim.ai stopped
answering `StartRun` and `OpenSubscription` while `GetRun` still did. A stream now returns the reason it
ended — `final`, `no frames` (stopped or failed before its first closed frame), `lease expired`,
`superseded` (a reconnect took over), `closed` (`CloseSubscription`), or `write error: <cause>` — and the
reason is logged; see the request log below.

The server holds at most eight runs, and a run counts toward the eight only while it is busy or leased:
unpaced-running, mid-`StepForward`, or paced with a live lease. An idle-stopped run — checkpoint written,
no lease — does not count, because the cap exists to bound CPU and memory for live work and a parked
checkpoint is neither. When a `StartRun` arrives and eight non-terminal runs are held, the server evicts
the oldest idle-stopped run (by registration order) to make room, and answers `503` only when there is
none to evict, so a crashed tab can never lock the public site. A run that stays idle-stopped for
`2 × IDLE_SHUTDOWN_SECONDS` is reaped on its own, so memory stays bounded with no new arrivals. Either
way the run is simply gone from the registry: `GetRun` answers `404` from then on, while its checkpoint
under `runs/<run_id>/` stays on disk. Finished and failed runs never count toward the eight.

A finished run — `STATE_COMPLETE` by its own end or by `StopRun`, or `STATE_FAILED` — is **released**
rather than reaped: it leaves memory once its checkpoint is on disk, it has no live lease, and
`LBSIM_COMPLETED_RETENTION_S` (default 120) have passed since it ended; or at once, checkpoint written and
lease-free, when a `StartRun` arrives with eight runs held in any state. Before this rule every finished
run stayed until the instance died, ninety-odd MB each at 256 replicas, which reached the engine's memory
budget in about ten dashboard runs. The checkpoint of a completed run is written the moment it ends, from
the frames the viewer saw: `status.json`, `scenario.txt`, `result.json`, `fleet.jsonl`, `replicas.jsonl`
thinned to the export's budget with the stride in `checkpoint.json`, and `traces.jsonl` with its
`manifest.json` when the run sampled any. After the release the server keeps a hundred-byte status stub:
`ListRuns` still lists the run and `GetRun` answers its final status; `GetResult` answers `result.json`
from disk, byte for byte the in-memory answer (`409` for a failed run, as before); everything that needs
the engine or the frames — `StopRun`, `SetSpeed`, `StepForward`, `UpdateWorkload`, `UpdatePolicies`,
`GetTraces`, `OpenSubscription` — answers `410` with `released; replay from runs/<run_id>/`. The
released run is not merged into `runs/index.json`, which lists the showcase's recordings, so a dashboard
replay of it is a follow-up rather than a promise.

While a run is alive the server holds its frames, one per sample interval with a row per replica, and
nothing per request: the engine folds each record into the whole-run tally and histograms as it lands
(`Sim::fold_records`), so a ten-minute run at 560 rps holds no 336k-record vector to its end. What
`/requests.log` reports (below) is therefore the frames.

`GET /requests.log` answers `text/plain` with the last 512 request lines, oldest first, because the
deploy identity cannot read Cloud Logging and a stall has to be diagnosable from the outside. One line
per ingress request, `req <method> <rpc> <status> <ms>ms [run=<id>] [sub=s-<n>]`; for a subscription,
`sse open sub=s-<n> run=<id>` once the head has gone out and `sse end sub=s-<n> reason=<reason> <ms>ms`
when the stream returns; and `busy 503 connections=<n>` whenever the accept loop sheds a connection.
Every line also goes to stderr with a wall-clock prefix, which Cloud Run captures. `/health` and the log
itself are not logged, and `/health` never touches run state. The log opens with a memory report, `#`
lines ahead of the request lines: `# rss_mb=<n> held_runs=<n> released_runs=<n> open_subscriptions=<n>
completed_retention_s=<n>` (the resident set from `/proc/self/statm`), then `# run <id> <STATE>
frames=<n> traces=<n>` per run in memory. A memory question about the public site starts by reading
these numbers rather than by guessing them.

## What `sim-run export` writes, and the decisions it settled

`sim-run export --demos --dir DIR` (and `export <scenario.txt ...>`) writes, under `DIR/runs/`:

```
runs/index.json                      one object per line, merged and sorted by run_id on re-export
runs/<group>/<run>/status.json       RunStatus, STATE_COMPLETE
runs/<group>/<run>/scenario.txt      the resolved flat scenario
runs/<group>/<run>/result.json       metrics.proto RunResult, verbatim: run_id, seed, event_count,
                                     state_checksum (the fingerprint), overall Scorecard
runs/<group>/<run>/fleet.jsonl       one SubscriptionUpdate per sample instant, SCOPE_FLEET, last has final
runs/<group>/<run>/replicas.jsonl    one SubscriptionUpdate per replica per sampled instant, SCOPE_REPLICA,
                                     ordered by sample then replica_id; every replica_sample_stride-th
                                     fleet.jsonl instant and the last one, last has final
```

Settled while building it, and binding on the server too:

- `index.json` is the one document with no proto: `{run_id, name, routing, scenario_file, sim_start_unix_ns,
  sim_end_unix_ns, sample_interval_ms, replicas, replica_sample_stride}`.
- A gauge that is undefined in a window (SLO attainment with no completions, imbalance on an idle fleet)
  and a distribution with `count` 0 are **omitted** from the maps, never written as 0.
- `METRIC_KV_UTILIZATION` is a **fraction** on the wire; the engine's series is a percentage.
- `METRIC_ADMITTED_RPS` equals completions per window until the engine has an admission series.
- Whole-run distributions in `result.json` carry `from_merged_histogram: true` (bucketed, accurate);
  windowed ones in `fleet.jsonl` are exact and carry `false`.
- `subscription_id` is `"export"` and `realtime_factor` is `0` in exported rows; the live server fills both.
- Per-replica rows (`SCOPE_REPLICA`) come from `RunResult.frames[].replicas` and go to `replicas.jsonl`,
  a file of their own so a reader that wants only the fleet charts never parses them. Absent in exports
  older than U23; a dashboard treats that as a run with no replica breakdown, not an error.
- `replicas.jsonl` is bounded to 8 MiB per run (`export::REPLICA_ROWS_BUDGET_BYTES`). A run that fits
  carries every fleet sample; one that does not carries every `replica_sample_stride`-th sample from the
  first, plus the last, with the smallest stride that fits, and the index says which. Cloud Run refuses
  a response with a `Content-Length` above 32 MiB, and the 256-replica demos had reached 40 MB. A
  dashboard reads a frame with no rows of its own as carrying the last sampled instant's rows while it
  is fewer than a stride behind; an index without the field is stride 1, which is what those exports
  were. The server also sends any static file above 4 MiB as `Transfer-Encoding: chunked`
  (`CHUNKED_THRESHOLD_BYTES`), so no document can hit the cap whatever its size.
