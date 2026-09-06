// Golden JSON fixtures for the Ingress transport, hand-written to `crates/sim-ingress/WIRE.md`.
//
// Written by hand rather than captured from the server on purpose: a fixture captured from the
// implementation tests that the client agrees with today's server, and this has to test that both
// agree with the protos and with WIRE.md. Field names are the proto names verbatim (snake_case),
// every uint64 is a decimal string, every enum is a name, every enum-keyed map uses the enum number
// as its key, and several messages deliberately omit zero-valued fields so the decoder is forced to
// supply proto3 defaults.
//
// The `WIRE_*` constants are WIRE.md's own examples, copied verbatim, so a change to that document
// that the client does not follow fails here.
//
// Stored as JSON *text*, so the parse is part of what is under test. `JSON.parse` on a uint64 string
// is the step where a careless client would produce a `number` and lose the low nanoseconds.

/** 2026-01-01T00:00:00Z in Unix epoch nanoseconds: the magnitude the whole time convention is about. */
export const EPOCH_NS = '1767225600000000000';
/** The same instant plus 123 ns: the detail a float64 cannot hold at this magnitude. */
export const EPOCH_NS_PLUS_123 = '1767225600000000123';

// ---------------------------------------------------------------------------
// WIRE.md's examples, verbatim
// ---------------------------------------------------------------------------

/** WIRE.md, "Messages, as JSON": RunStatus. */
export const WIRE_RUN_STATUS = `{ "run_id": "r-1", "state": "STATE_RUNNING", "sim_time_unix_ns": "1767225615000000000",
  "sim_end_unix_ns": "1767225720000000000", "realtime_factor": 2.0, "error": "" }`;

/** WIRE.md, "The one deviation from the proto: the scenario": a StartRunRequest body. */
export const WIRE_START_RUN_REQUEST = `{
  "scenario": { "text": "name = p2c\\nseed = 20260906\\n...", "overrides": { "arrival_rps": "90" } },
  "max_realtime_factor": 0,
  "record_traces": false
}`;

/** WIRE.md: the first SSE event, an OpenSubscriptionResponse. */
export const WIRE_OPEN_EVENT = `event: open
data: {"subscription_id":"s-7","lease_expires_at_wall_ns":"1767225660000000000"}

`;

/** WIRE.md: every later SSE event, a SubscriptionUpdate, with the `id:` the reconnect rule requires. */
export const WIRE_UPDATE_EVENT = `id: 1
event: update
data: {"subscription_id":"s-7","sim_time_unix_ns":"1767225615000000000","realtime_factor":2.0,
data:        "row":{"target":{"scope":"SCOPE_FLEET"},
data:               "values":{"40":70.0,"23":12.0},
data:               "distributions":{"1":{"count":"41","mean":812.5,"min":120000000.0,"max":2400000000.0,
data:                                     "percentile":[50,99],"value":[700000000.0,2100000000.0],
data:                                     "from_merged_histogram":false}}},
data:        "final":false}

`;

/** WIRE.md, "Encoding rules" 6: an error body. */
export const WIRE_ERROR_BODY = `{"error": "run r-9 is not running"}`;

/** WIRE.md: an UpdateWorkload / UpdatePolicies body. */
export const WIRE_UPDATE_REQUEST = `{"run_id": "r-1", "overrides": {"arrival_rps": "90"}}`;

// ---------------------------------------------------------------------------
// Unary responses
// ---------------------------------------------------------------------------

export const START_RUN_RESPONSE = `{"run_id":"r-1"}`;

export const RUN_STATUS = `{
  "run_id": "r-1",
  "state": "STATE_RUNNING",
  "sim_time_unix_ns": "${EPOCH_NS_PLUS_123}",
  "sim_end_unix_ns": "1767225720000000000",
  "realtime_factor": 2.5,
  "error": ""
}`;

/** Everything zero-valued omitted, which WIRE.md rule 5 says the server does. */
export const RUN_STATUS_SPARSE = `{"run_id":"r-2","state":"STATE_QUEUED"}`;

export const LIST_RUNS_RESPONSE = `{
  "runs": [
    {"run_id":"r-1","state":"STATE_COMPLETE","sim_time_unix_ns":"${EPOCH_NS}","sim_end_unix_ns":"${EPOCH_NS}","realtime_factor":0},
    {"run_id":"r-2","state":"STATE_FAILED","error":"replica pool exhausted"}
  ],
  "next_cursor": ""
}`;

export const REWIND_RESPONSE = `{"sim_time_unix_ns":"1767225630000000000","from_log":true,"restored_from_snapshot_unix_ns":"0"}`;

export const REWIND_RESPONSE_RESIMULATED = `{"sim_time_unix_ns":"1767225630000000000","restored_from_snapshot_unix_ns":"1767225620000000000"}`;

export const UPDATE_RESPONSE = `{"accepted":true,"required_resimulation":true,"rewound_to_unix_ns":"1767225620000000000","rejected_reason":""}`;

export const UPDATE_RESPONSE_REJECTED = `{"rejected_reason":"arrival_rps must be positive"}`;

export const OPEN_SUBSCRIPTION_RESPONSE = `{"subscription_id":"sub-7","lease_expires_at_wall_ns":"1767225660000000000","rejected_reason":""}`;

/** The name an earlier contract used. It must decode to the default, not be silently read. */
export const OPEN_SUBSCRIPTION_RESPONSE_OLD_NAME = `{"subscription_id":"sub-7","lease_expires_at_unix_ns":"1767225660000000000"}`;

export const OPEN_SUBSCRIPTION_REJECTED = `{"subscription_id":"","rejected_reason":"SCOPE_REPLICA needs replica_id"}`;

export const RENEW_RESPONSE = `{"lease_expires_at_wall_ns":"1767225720000000000","expired":false}`;

export const RENEW_RESPONSE_EXPIRED = `{"expired":true}`;

export const CLOSE_SUBSCRIPTION_RESPONSE = `{}`;

/**
 * A fleet update. `values` is keyed by Metric number: 23 is METRIC_QUEUED_SEQS, 40
 * METRIC_OFFERED_RPS, 45 METRIC_GOODPUT_TOKENS_PER_S, 64 METRIC_LOAD_IMBALANCE_CV. `distributions`
 * carries 1, METRIC_TTFT. 999 is a metric this client does not know, and must not crash on.
 */
export const SUBSCRIPTION_UPDATE_FLEET = `{
  "subscription_id": "sub-7",
  "sim_time_unix_ns": "${EPOCH_NS_PLUS_123}",
  "realtime_factor": 2.5,
  "row": {
    "target": {"scope":"SCOPE_FLEET"},
    "values": {"23": 4.0, "40": 70.5, "45": 18400.25, "64": 0.31, "999": 1.5},
    "distributions": {
      "1": {
        "count": "18422",
        "mean": 412500000.0,
        "min": 88000000.0,
        "max": 9120500000.5,
        "percentile": [50, 90, 99, 99.9],
        "value": [310000000.0, 780500000.0, 2400000000.0, 8800000000.0],
        "from_merged_histogram": true
      }
    }
  },
  "final": false
}`;

/** A replica update: one id field, and the final flag set. */
export const SUBSCRIPTION_UPDATE_REPLICA = `{
  "subscription_id": "sub-8",
  "sim_time_unix_ns": "1767225660000000000",
  "row": {
    "target": {"scope":"SCOPE_REPLICA","replica_id":"3"},
    "values": {"20": 0.82, "22": 27}
  },
  "final": true
}`;

export const RUN_RESULT = `{
  "run_id": "r-1",
  "seed": "20260906",
  "event_count": "48211904",
  "state_checksum": "12297829382473034410",
  "overall": {
    "values": {"45": 18400.25, "66": 0.972, "65": 0.11},
    "distributions": {
      "3": {"count":"2100311","mean":8400000000.0,"min":210000000.0,"max":61000000000.0,"percentile":[99],"value":[42000000000.0]}
    },
    "outcome_counts": {"1": "2038112", "2": "41200", "3": "20999", "5": "12"},
    "declared_rated_capacity_rps": 68.4,
    "metastable_collapse": false,
    "recovery_time_ns": "0"
  },
  "by_scope": [
    {"target":{"scope":"SCOPE_REPLICA","replica_id":"3"},"scorecard":{"values":{"20":0.79}}}
  ],
  "recorded": [
    {"subscription_id":"","sim_time_unix_ns":"${EPOCH_NS}","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":70.0}}}
  ],
  "traces": [],
  "referee_violations": {"3": "4"},
  "wall_clock_seconds": 12.5,
  "realtime_factor": 9.6
}`;

export const GET_TRACES_RESPONSE = `{
  "traces": [
    {
      "record": {
        "id": "8891234",
        "tenant_id": "2",
        "slo_class": "SLO_CLASS_INTERACTIVE",
        "outcome": "OUTCOME_OK_SLO_VIOLATED",
        "arrived_at_unix_ns": "${EPOCH_NS}",
        "admitted_at_unix_ns": "1767225600004000000",
        "first_token_at_unix_ns": "1767225602110000000",
        "finished_at_unix_ns": "1767225611900000000",
        "prompt_tokens": 24310,
        "output_tokens": 402,
        "cached_prefix_tokens": 1120,
        "replica_id": "17",
        "replica_path_id": ["17","4"],
        "attempts": 1,
        "preemptions": 2,
        "queue_wait_ns": "4000000",
        "preempted_ns": "1900000000",
        "ttft_ns": "2110000000",
        "e2e_ns": "11900000000",
        "mean_itl_ns": "24000000",
        "p99_itl_ns": "310000000"
      },
      "spans": [
        {"start_unix_ns":"${EPOCH_NS}","end_unix_ns":"1767225600004000000","component":"gateway","operation":"queue","concurrent_seqs":31,"kv_utilization":0.78},
        {"start_unix_ns":"1767225600004000000","end_unix_ns":"1767225602110000000","component":"replica:17","operation":"prefill","replica_id":"17","tokens_processed":23190,"kv_tier":"MEMORY_TIER_HBM"}
      ]
    }
  ]
}`;

// ---------------------------------------------------------------------------
// Server-sent events
// ---------------------------------------------------------------------------

/**
 * One SSE stream, exactly as WIRE.md frames it: a comment keepalive, the `open` event, then
 * `update` events each carrying `id: <n>` from 1, ending with the `final` update.
 */
export const SSE_STREAM = [
  `: keepalive\n\n`,
  `event: open\ndata: {"subscription_id":"sub-7","lease_expires_at_wall_ns":"1767225660000000000"}\n\n`,
  `id: 1\nevent: update\ndata: {"subscription_id":"sub-7","sim_time_unix_ns":"${EPOCH_NS}","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":70.0}}}\n\n`,
  `id: 2\nevent: update\ndata: {"subscription_id":"sub-7","sim_time_unix_ns":"1767225600500000000","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":71.5}}}\n\n`,
  `: keepalive\n\n`,
  `id: 3\nevent: update\ndata: {"subscription_id":"sub-7","sim_time_unix_ns":"1767225601000000000","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":69.0}},"final":true}\n\n`,
].join('');

/** The same three updates without the final flag, for a stream the server closes mid-run. */
export const SSE_STREAM_OPEN_ENDED = SSE_STREAM.replace(',"final":true', '');

/** A stream the server sends after a reconnect: no `open` event, the replayed updates after the given id. */
export function sseReplay(afterId: number, count: number): string {
  let out = '';
  for (let i = 1; i <= count; i++) {
    const id = afterId + i;
    out += `id: ${id}\nevent: update\ndata: {"subscription_id":"sub-7","sim_time_unix_ns":"${BigInt(EPOCH_NS) + BigInt(id) * 500_000_000n}","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":${70 + id}}}}\n\n`;
  }
  return out;
}

/** An `open` event the server uses to reject: HTTP 200, `rejected_reason` set, per WIRE.md rule 6. */
export const SSE_REJECTED = `event: open\ndata: ${OPEN_SUBSCRIPTION_REJECTED}\n\n`;

/**
 * The same stream chopped where a real socket would chop it: inside a JSON object, between the
 * `data:` prefix and its payload, and between the two newlines that end a frame. A client that
 * assumes a chunk is a frame passes the fixture above and fails this one.
 */
export function chopStream(text: string, sizes: number[]): string[] {
  const chunks: string[] = [];
  let i = 0;
  let k = 0;
  while (i < text.length) {
    const n = sizes[k % sizes.length];
    chunks.push(text.slice(i, i + n));
    i += n;
    k++;
  }
  return chunks;
}

/** Awkward on purpose: 7 is coprime with nothing in the frame lengths, so boundaries land mid-token. */
export const SSE_CHUNK_SIZES = [7, 13, 1, 97, 3];

/** A stream that ends mid-frame, which is what an aborted connection looks like. */
export const SSE_TRUNCATED = `id: 4\nevent: update\ndata: {"subscription_id":"sub-7","sim_time_unix_ns":"${EPOCH_NS}"`;
