// Golden proto3-JSON fixtures for the Ingress transport, hand-written to the wire contract.
//
// Written by hand rather than captured from the server on purpose: a fixture captured from the
// implementation tests that the client agrees with today's server, and this has to test that both
// agree with the protos. Every 64-bit field is a string, every enum is a name, every map key is a
// stringified enum number, and several messages deliberately omit zero-valued fields so the decoder
// is forced to supply proto3 defaults.
//
// Stored as JSON *text*, so the parse is part of what is under test. `JSON.parse` on a uint64 string
// is the step where a careless client would produce a `number` and lose the low nanoseconds.

/** 2026-01-01T00:00:00Z in Unix epoch nanoseconds: the magnitude the whole time convention is about. */
export const EPOCH_NS = '1767225600000000000';
/** The same instant plus 123 ns: the detail a float64 cannot hold at this magnitude. */
export const EPOCH_NS_PLUS_123 = '1767225600000000123';

export const START_RUN_RESPONSE = `{"runId":"r-1"}`;

export const RUN_STATUS = `{
  "runId": "r-1",
  "state": "STATE_RUNNING",
  "simTimeUnixNs": "${EPOCH_NS_PLUS_123}",
  "simEndUnixNs": "1767225720000000000",
  "realtimeFactor": 2.5,
  "error": ""
}`;

/** Everything zero-valued omitted, which proto3 JSON is entitled to do. */
export const RUN_STATUS_SPARSE = `{"runId":"r-2","state":"STATE_QUEUED"}`;

export const LIST_RUNS_RESPONSE = `{
  "runs": [
    {"runId":"r-1","state":"STATE_COMPLETE","simTimeUnixNs":"${EPOCH_NS}","simEndUnixNs":"${EPOCH_NS}","realtimeFactor":0},
    {"runId":"r-2","state":"STATE_FAILED","error":"replica pool exhausted"}
  ],
  "nextCursor": ""
}`;

export const REWIND_RESPONSE = `{"simTimeUnixNs":"1767225630000000000","fromLog":true,"restoredFromSnapshotUnixNs":"0"}`;

export const REWIND_RESPONSE_RESIMULATED = `{"simTimeUnixNs":"1767225630000000000","restoredFromSnapshotUnixNs":"1767225620000000000"}`;

export const UPDATE_RESPONSE = `{"accepted":true,"requiredResimulation":true,"rewoundToUnixNs":"1767225620000000000","rejectedReason":""}`;

export const UPDATE_RESPONSE_REJECTED = `{"rejectedReason":"arrival_rps must be positive"}`;

export const OPEN_SUBSCRIPTION_RESPONSE = `{"subscriptionId":"sub-7","leaseExpiresAtUnixNs":"1767225660000000000","rejectedReason":""}`;

export const RENEW_RESPONSE = `{"leaseExpiresAtUnixNs":"1767225720000000000","expired":false}`;

export const RENEW_RESPONSE_EXPIRED = `{"expired":true}`;

export const CLOSE_SUBSCRIPTION_RESPONSE = `{}`;

/**
 * A fleet update. `values` is a map<int32,double> keyed by Metric number: 23 is METRIC_QUEUED_SEQS,
 * 40 METRIC_OFFERED_RPS, 45 METRIC_GOODPUT_TOKENS_PER_S, 64 METRIC_LOAD_IMBALANCE_CV. `distributions`
 * carries 1, METRIC_TTFT. 999 is a metric this client does not know, and must not crash on.
 */
export const SUBSCRIPTION_UPDATE_FLEET = `{
  "subscriptionId": "sub-7",
  "simTimeUnixNs": "${EPOCH_NS_PLUS_123}",
  "realtimeFactor": 2.5,
  "row": {
    "target": {"scope":"SCOPE_FLEET"},
    "values": {"23": 4.0, "40": 70.5, "45": 18400.25, "64": 0.31, "999": 1.5},
    "distributions": {
      "1": {
        "count": "18422",
        "mean": 412.5,
        "min": 88.0,
        "max": 9120.5,
        "percentile": [50, 90, 99, 99.9],
        "value": [310.0, 780.5, 2400.0, 8800.0],
        "fromMergedHistogram": true
      }
    }
  },
  "final": false
}`;

/** A replica update: one id field, and the final flag set. */
export const SUBSCRIPTION_UPDATE_REPLICA = `{
  "subscriptionId": "sub-8",
  "simTimeUnixNs": "1767225660000000000",
  "row": {
    "target": {"scope":"SCOPE_REPLICA","replicaId":"3"},
    "values": {"20": 0.82, "22": 27}
  },
  "final": true
}`;

export const RUN_RESULT = `{
  "runId": "r-1",
  "seed": "20260906",
  "eventCount": "48211904",
  "stateChecksum": "12297829382473034410",
  "overall": {
    "values": {"45": 18400.25, "66": 0.972, "65": 0.11},
    "distributions": {
      "3": {"count":"2100311","mean":8400.0,"min":210.0,"max":61000.0,"percentile":[99],"value":[42000.0]}
    },
    "outcomeCounts": {"1": "2038112", "2": "41200", "3": "20999", "5": "12"},
    "declaredRatedCapacityRps": 68.4,
    "metastableCollapse": false,
    "recoveryTimeNs": "0"
  },
  "byScope": [
    {"target":{"scope":"SCOPE_REPLICA","replicaId":"3"},"scorecard":{"values":{"20":0.79}}}
  ],
  "recorded": [
    {"subscriptionId":"","simTimeUnixNs":"${EPOCH_NS}","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":70.0}}}
  ],
  "traces": [],
  "refereeViolations": {"3": "4"},
  "wallClockSeconds": 12.5,
  "realtimeFactor": 9.6
}`;

export const GET_TRACES_RESPONSE = `{
  "traces": [
    {
      "record": {
        "id": "8891234",
        "tenantId": "2",
        "sloClass": "SLO_CLASS_INTERACTIVE",
        "outcome": "OUTCOME_OK_SLO_VIOLATED",
        "arrivedAtUnixNs": "${EPOCH_NS}",
        "admittedAtUnixNs": "1767225600004000000",
        "firstTokenAtUnixNs": "1767225602110000000",
        "finishedAtUnixNs": "1767225611900000000",
        "promptTokens": 24310,
        "outputTokens": 402,
        "cachedPrefixTokens": 1120,
        "replicaId": "17",
        "replicaPathId": ["17","4"],
        "attempts": 1,
        "preemptions": 2,
        "queueWaitNs": "4000000",
        "preemptedNs": "1900000000",
        "ttftNs": "2110000000",
        "e2eNs": "11900000000",
        "meanItlNs": "24000000",
        "p99ItlNs": "310000000"
      },
      "spans": [
        {"startUnixNs":"${EPOCH_NS}","endUnixNs":"1767225600004000000","component":"gateway","operation":"queue","concurrentSeqs":31,"kvUtilization":0.78},
        {"startUnixNs":"1767225600004000000","endUnixNs":"1767225602110000000","component":"replica:17","operation":"prefill","replicaId":"17","tokensProcessed":23190,"kvTier":"MEMORY_TIER_HBM"}
      ]
    }
  ]
}`;

export const ERROR_BODY = `{"error":{"code":9,"message":"run r-9 is not running"}}`;

/**
 * One SSE stream, exactly as the server writes it: a comment keepalive, then frames, each
 * `data: <json>` followed by a blank line.
 */
export const SSE_STREAM = [
  `: keepalive\n\n`,
  `data: {"subscriptionId":"sub-7","simTimeUnixNs":"${EPOCH_NS}","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":70.0}}}\n\n`,
  `data: {"subscriptionId":"sub-7","simTimeUnixNs":"1767225600500000000","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":71.5}}}\n\n`,
  `: keepalive\n\n`,
  `data: {"subscriptionId":"sub-7","simTimeUnixNs":"1767225601000000000","row":{"target":{"scope":"SCOPE_FLEET"},"values":{"40":69.0}},"final":true}\n\n`,
].join('');

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
export const SSE_TRUNCATED = `data: {"subscriptionId":"sub-7","simTimeUnixNs":"${EPOCH_NS}"`;
