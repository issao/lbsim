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
policy keys respectively; the server rejects a key from the wrong group with 400. Both are applied by
restoring the most recent snapshot and re-simulating, so `UpdateResponse.required_resimulation` is
`true` and `rewound_to_unix_ns` says where.

## Messages, as JSON

`RunStatus`:
```json
{ "run_id": "r-1", "state": "STATE_RUNNING", "sim_time_unix_ns": "1767225615000000000",
  "sim_end_unix_ns": "1767225720000000000", "realtime_factor": 2.0, "error": "" }
```

`OpenSubscription` query parameters, mirroring `OpenSubscriptionRequest`:
`run_id`, `scope` (`SCOPE_FLEET` | `SCOPE_REPLICA`), `replica_id` (required for `SCOPE_REPLICA`),
`metrics` (comma-separated `METRIC_*` names), `samples_per_sim_second` (float), `percentiles`
(comma-separated floats), `lease_ns` (wall-clock, decimal string).

First event, `OpenSubscriptionResponse`:
```
event: open
data: {"subscription_id":"s-7","lease_expires_at_unix_ns":"..."}
```

Every later event, `SubscriptionUpdate`:
```
event: update
data: {"subscription_id":"s-7","sim_time_unix_ns":"...","realtime_factor":2.0,
       "row":{"target":{"scope":"SCOPE_FLEET"},
              "values":{"40":70.0,"23":12.0},
              "distributions":{"1":{"count":"41","mean":812.5,"min":"120000000","max":"2400000000",
                                    "percentile":[50,99],"value":[700000000.0,2100000000.0],
                                    "from_merged_histogram":false}}},
       "final":false}
```
Distribution `min`/`max` are `uint64` nanoseconds and therefore strings; `mean` and `value[]` are
doubles in nanoseconds. A distribution at time `t` describes the requests that **completed in the
sample window ending at `t`**, one window per `1 / samples_per_sim_second` simulated seconds, so a chart
of p99 is a chart of the recent tail, not a cumulative one. `count` says how many that was.

## What the first server supports

Scopes: `SCOPE_FLEET` and `SCOPE_REPLICA`. Everything else returns `rejected_reason`.

| Metric | Fleet | Replica | Source in the engine |
|---|---|---|---|
| `METRIC_OFFERED_RPS` 40 | yes | | `Workload::rate_at` |
| `METRIC_COMPLETED_RPS` 42, `METRIC_ADMITTED_RPS` 41, `METRIC_REJECTED_RPS` 43 | yes | | completions per sample window |
| `METRIC_OUTPUT_TOKENS_PER_S` 44, `METRIC_GOODPUT_TOKENS_PER_S` 45 | yes | | tokens of completions per window, all / within SLO |
| `METRIC_QUEUED_SEQS` 23, `METRIC_RUNNING_SEQS` 22 | sum | yes | replica queue and batch |
| `METRIC_KV_UTILIZATION` 20, `METRIC_KV_TOKENS_RESIDENT` 21 | mean / sum | yes | replica KV |
| `METRIC_STEP_TIME` 8 | | yes | last step duration |
| `METRIC_LOAD_IMBALANCE_CV` 64 | yes | | CV of per-replica load at the sample |
| `METRIC_SLO_ATTAINMENT` 66 | yes | | window completions within SLO / all window completions |
| `METRIC_TTFT` 1, `METRIC_ITL` 2, `METRIC_E2E` 3, `METRIC_QUEUE_WAIT` 4 | yes | | window histograms |
| `METRIC_READY_REPLICAS` 60 | yes | | replica count, until lifecycle exists |

Sample cadence: the server honours `samples_per_sim_second` as asked. The engine records at the
scenario's `sample_interval_ms`; a subscription rate finer than that gets the nearest recorded sample
repeated, and the response's `sim_time_unix_ns` is the recorded sample's time, never an interpolated
one, because an interpolated batch composition never existed.

## Leases and idle shutdown

`lease_ns` is wall-clock. The server drops a subscription whose lease expired without renewal and ends
its stream. A run with no live lease and no queued work for `IDLE_SHUTDOWN_SECONDS` (default 300)
checkpoints and stops advancing, so a Cloud Run instance can be reaped; `GetRun` on such a run reports
`STATE_PAUSED` with `error` empty. Reopening a subscription resumes it. These are `lease.rs` and
`idle.rs` in this crate.
