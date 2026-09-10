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

// ---------------------------------------------------------------------------
// A live stream's worth of a real run: twenty consecutive rows of the p2c demo's fleet.jsonl
// ---------------------------------------------------------------------------
//
// Lines 100-119 of `runs/1-routing/p2c/fleet.jsonl` from `sim-run export --demos`, 25.0 s to 29.75 s
// of simulated time at 4 samples per second, verbatim. The fake Ingress server serves these as SSE
// updates, so the live path is checked against the same engine numbers replay is checked against:
// a frame decoded from the stream must equal the frame decoded from the file.

export const FLEET_EXCERPT_ORIGIN = '1767225600000000000';
export const FLEET_EXCERPT_ROWS = 20;

export const FLEET_JSONL_EXCERPT = `{"subscription_id":"export","sim_time_unix_ns":"1767225625000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04451532846715328,"22":315,"23":1,"40":70,"41":64,"42":64,"43":0,"44":12940,"45":12940,"60":32,"64":0.29298860493540535,"66":1},"distributions":{"1":{"count":"16","mean":236793156.3125,"min":28028807,"max":1628442058,"percentile":[50,90,99,99.9],"value":[51747847,568413189,1628442058,1628442058],"from_merged_histogram":false},"2":{"count":"16","mean":47268471.875,"min":43820167,"max":50123641,"percentile":[50,90,99,99.9],"value":[47063609,49206676,50123641,50123641],"from_merged_histogram":false},"3":{"count":"16","mean":3193849112.0625,"min":254467178,"max":8653270206,"percentile":[50,90,99,99.9],"value":[1958531795,6334394106,8653270206,8653270206],"from_merged_histogram":false},"4":{"count":"16","mean":8374596,"min":231221,"max":29873566,"percentile":[50,90,99,99.9],"value":[6294777,19754572,29873566,29873566],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225625250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.042328398722627746,"22":297,"23":1,"40":70,"41":128,"42":128,"43":0,"44":20424,"45":20224,"60":32,"64":0.309110520867707,"66":0.96875},"distributions":{"1":{"count":"32","mean":299628488.875,"min":21195752,"max":2137977021,"percentile":[50,90,99,99.9],"value":[68332378,1236062654,2137977021,2137977021],"from_merged_histogram":false},"2":{"count":"32","mean":44327664.1875,"min":10675282,"max":50123641,"percentile":[50,90,99,99.9],"value":[46922156,47868836,50123641,50123641],"from_merged_histogram":false},"3":{"count":"32","mean":2609118145.625,"min":250168761,"max":8406116792,"percentile":[50,90,99,99.9],"value":[1972678755,5363122230,8406116792,8406116792],"from_merged_histogram":false},"4":{"count":"32","mean":11029262.5,"min":1186516,"max":48117488,"percentile":[50,90,99,99.9],"value":[6156233,28687454,48117488,48117488],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225625500000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.0437960310218978,"22":294,"23":2,"40":70,"41":64,"42":64,"43":0,"44":14228,"45":12440,"60":32,"64":0.3237425761890197,"66":0.9375},"distributions":{"1":{"count":"16","mean":411985251.375,"min":18916099,"max":2891038681,"percentile":[50,90,99,99.9],"value":[37172171,1291698791,2891038681,2891038681],"from_merged_histogram":false},"2":{"count":"16","mean":46059999.9375,"min":25560812,"max":50572656,"percentile":[50,90,99,99.9],"value":[46903309,48597081,50572656,50572656],"from_merged_histogram":false},"3":{"count":"16","mean":3698057915.0625,"min":678417212,"max":10317201102,"percentile":[50,90,99,99.9],"value":[2416633008,8169287436,10317201102,10317201102],"from_merged_histogram":false},"4":{"count":"16","mean":9519429.3125,"min":398934,"max":43318227,"percentile":[50,90,99,99.9],"value":[5500018,27854144,43318227,43318227],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225625750000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.044472764598540146,"22":302,"23":1,"40":70,"41":48,"42":48,"43":0,"44":5436,"45":4900,"60":32,"64":0.31396194943572725,"66":0.8333333333333334},"distributions":{"1":{"count":"12","mean":627567072.6666666,"min":31407749,"max":3407869120,"percentile":[50,90,99,99.9],"value":[86067016,2286447522,3407869120,3407869120],"from_merged_histogram":false},"2":{"count":"12","mean":44052000.75,"min":13824460,"max":49141856,"percentile":[50,90,99,99.9],"value":[46903309,48509144,49141856,49141856],"from_merged_histogram":false},"3":{"count":"12","mean":2460166813,"min":510050425,"max":6585021241,"percentile":[50,90,99,99.9],"value":[1136132038,5405037155,6585021241,6585021241],"from_merged_histogram":false},"4":{"count":"12","mean":12023247,"min":973821,"max":41902660,"percentile":[50,90,99,99.9],"value":[8205949,28200076,41902660,41902660],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225626000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04796840784671533,"22":304,"23":0,"40":70,"41":60,"42":60,"43":0,"44":17200,"45":17200,"60":32,"64":0.310654343266083,"66":1},"distributions":{"1":{"count":"15","mean":95764740.8,"min":16697435,"max":297451776,"percentile":[50,90,99,99.9],"value":[56497869,191119766,297451776,297451776],"from_merged_histogram":false},"2":{"count":"15","mean":41288981.8,"min":11276425,"max":49206676,"percentile":[50,90,99,99.9],"value":[47475419,48180389,49206676,49206676],"from_merged_histogram":false},"3":{"count":"15","mean":4254644735.266667,"min":406014249,"max":10539996379,"percentile":[50,90,99,99.9],"value":[2713606939,9767909023,10539996379,10539996379],"from_merged_histogram":false},"4":{"count":"15","mean":8610140.4,"min":1415883,"max":38104732,"percentile":[50,90,99,99.9],"value":[6160375,17316507,38104732,38104732],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225626250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04822917427007299,"22":312,"23":0,"40":70,"41":48,"42":48,"43":0,"44":11060,"45":11060,"60":32,"64":0.32328823160446274,"66":1},"distributions":{"1":{"count":"12","mean":86212194.5,"min":19828234,"max":339423796,"percentile":[50,90,99,99.9],"value":[37753782,190028768,339423796,339423796],"from_merged_histogram":false},"2":{"count":"12","mean":44524929.166666664,"min":10714010,"max":49206676,"percentile":[50,90,99,99.9],"value":[47475419,48132264,49206676,49206676],"from_merged_histogram":false},"3":{"count":"12","mean":3554153906.3333335,"min":445650271,"max":9106330452,"percentile":[50,90,99,99.9],"value":[1897170116,6958940080,9106330452,9106330452],"from_merged_histogram":false},"4":{"count":"12","mean":5466633.25,"min":276928,"max":8915972,"percentile":[50,90,99,99.9],"value":[5771785,8084880,8915972,8915972],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225626500000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.048204744525547454,"22":319,"23":2,"40":70,"41":40,"42":40,"43":0,"44":8376,"45":8376,"60":32,"64":0.3147178508842961,"66":1},"distributions":{"1":{"count":"10","mean":350690898.9,"min":39708455,"max":1157589908,"percentile":[50,90,99,99.9],"value":[173671164,996772480,1157589908,1157589908],"from_merged_histogram":false},"2":{"count":"10","mean":45223596.9,"min":22764712,"max":50123641,"percentile":[50,90,99,99.9],"value":[47390579,47808864,50123641,50123641],"from_merged_histogram":false},"3":{"count":"10","mean":3398908445.7,"min":441306038,"max":7669518636,"percentile":[50,90,99,99.9],"value":[2696882091,6072089166,7669518636,7669518636],"from_merged_histogram":false},"4":{"count":"10","mean":12133034.8,"min":850404,"max":43217314,"percentile":[50,90,99,99.9],"value":[8805146,18916271,43217314,43217314],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225626750000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.048964165145985394,"22":319,"23":1,"40":70,"41":52,"42":52,"43":0,"44":9104,"45":9104,"60":32,"64":0.3058568081582854,"66":1},"distributions":{"1":{"count":"13","mean":167411069.53846154,"min":22194031,"max":825788318,"percentile":[50,90,99,99.9],"value":[45572850,533365774,825788318,825788318],"from_merged_histogram":false},"2":{"count":"13","mean":41065547.384615384,"min":10687637,"max":48036959,"percentile":[50,90,99,99.9],"value":[46968986,47850951,48036959,48036959],"from_merged_histogram":false},"3":{"count":"13","mean":2599757280.8461537,"min":205010515,"max":6067815012,"percentile":[50,90,99,99.9],"value":[2333274745,6026315839,6067815012,6067815012],"from_merged_histogram":false},"4":{"count":"13","mean":10420310.923076924,"min":965650,"max":36073083,"percentile":[50,90,99,99.9],"value":[6930704,33541572,36073083,36073083],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225627000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04879046532846716,"22":321,"23":6,"40":70,"41":72,"42":72,"43":0,"44":16080,"45":16080,"60":32,"64":0.28744708099274807,"66":1},"distributions":{"1":{"count":"18","mean":291825208.8333333,"min":33210055,"max":1190260102,"percentile":[50,90,99,99.9],"value":[66211775,1141439684,1190260102,1190260102],"from_merged_histogram":false},"2":{"count":"18","mean":47064381.55555555,"min":41678700,"max":48597081,"percentile":[50,90,99,99.9],"value":[47351799,48180389,48597081,48597081],"from_merged_histogram":false},"3":{"count":"18","mean":3983379379.8333335,"min":432312983,"max":10996414423,"percentile":[50,90,99,99.9],"value":[2562835997,10117257981,10996414423,10996414423],"from_merged_histogram":false},"4":{"count":"18","mean":10338222.555555556,"min":154431,"max":46379086,"percentile":[50,90,99,99.9],"value":[7893577,25614460,46379086,46379086],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225627250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04899596259124087,"22":324,"23":0,"40":70,"41":44,"42":44,"43":0,"44":6908,"45":6908,"60":32,"64":0.2879482417207556,"66":1},"distributions":{"1":{"count":"11","mean":444729844.72727275,"min":18987628,"max":1619499547,"percentile":[50,90,99,99.9],"value":[115083732,1439360839,1619499547,1619499547],"from_merged_histogram":false},"2":{"count":"11","mean":40736447,"min":10614155,"max":49561489,"percentile":[50,90,99,99.9],"value":[47597831,48535761,49561489,49561489],"from_merged_histogram":false},"3":{"count":"11","mean":3127715363.2727275,"min":366829724,"max":7126063373,"percentile":[50,90,99,99.9],"value":[2608178095,6008877419,7126063373,7126063373],"from_merged_histogram":false},"4":{"count":"11","mean":8557211.363636363,"min":845522,"max":25741673,"percentile":[50,90,99,99.9],"value":[7501636,16793341,25741673,25741673],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225627500000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04837659671532848,"22":314,"23":0,"40":70,"41":60,"42":60,"43":0,"44":7600,"45":7600,"60":32,"64":0.2978927736908245,"66":1},"distributions":{"1":{"count":"15","mean":175653419.8,"min":20371731,"max":1377799920,"percentile":[50,90,99,99.9],"value":[62061754,200363085,1377799920,1377799920],"from_merged_histogram":false},"2":{"count":"15","mean":41945975.06666667,"min":18693176,"max":50139426,"percentile":[50,90,99,99.9],"value":[47009656,48313774,50139426,50139426],"from_merged_histogram":false},"3":{"count":"15","mean":2575540031.4,"min":588515160,"max":11116250384,"percentile":[50,90,99,99.9],"value":[1863623410,6190989422,11116250384,11116250384],"from_merged_histogram":false},"4":{"count":"15","mean":8119388.066666666,"min":928423,"max":29888753,"percentile":[50,90,99,99.9],"value":[4170572,26533749,29888753,29888753],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225627750000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04918407846715328,"22":312,"23":1,"40":70,"41":64,"42":64,"43":0,"44":15436,"45":15436,"60":32,"64":0.3124105106438709,"66":1},"distributions":{"1":{"count":"16","mean":213154637.25,"min":24747313,"max":1427948761,"percentile":[50,90,99,99.9],"value":[52468523,896553544,1427948761,1427948761],"from_merged_histogram":false},"2":{"count":"16","mean":40365395.4375,"min":10807757,"max":48036959,"percentile":[50,90,99,99.9],"value":[46986819,48036959,48036959,48036959],"from_merged_histogram":false},"3":{"count":"16","mean":3702995509.75,"min":579906703,"max":9482209254,"percentile":[50,90,99,99.9],"value":[2672695628,8806358417,9482209254,9482209254],"from_merged_histogram":false},"4":{"count":"16","mean":7259773.9375,"min":3172814,"max":20221099,"percentile":[50,90,99,99.9],"value":[6591723,13048133,20221099,20221099],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225628000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04878695255474451,"22":325,"23":0,"40":70,"41":44,"42":44,"43":0,"44":10080,"45":9332,"60":32,"64":0.299181552019683,"66":0.9090909090909091},"distributions":{"1":{"count":"11","mean":625945017,"min":28639300,"max":3376388503,"percentile":[50,90,99,99.9],"value":[118712278,1996396038,3376388503,3376388503],"from_merged_histogram":false},"2":{"count":"11","mean":43453195.63636363,"min":10787632,"max":49918419,"percentile":[50,90,99,99.9],"value":[47475191,49222374,49918419,49918419],"from_merged_histogram":false},"3":{"count":"11","mean":4440067987.090909,"min":1272191384,"max":11387196385,"percentile":[50,90,99,99.9],"value":[3453330599,7953699503,11387196385,11387196385],"from_merged_histogram":false},"4":{"count":"11","mean":12212701.090909092,"min":1071187,"max":28722160,"percentile":[50,90,99,99.9],"value":[9039918,26159119,28722160,28722160],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225628250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04670558850364964,"22":319,"23":0,"40":70,"41":68,"42":68,"43":0,"44":12128,"45":8876,"60":32,"64":0.31566434579097336,"66":0.8823529411764706},"distributions":{"1":{"count":"17","mean":828428503.117647,"min":42863435,"max":5611110877,"percentile":[50,90,99,99.9],"value":[189315486,2130588912,5611110877,5611110877],"from_merged_histogram":false},"2":{"count":"17","mean":38005180.5882353,"min":10444440,"max":50572656,"percentile":[50,90,99,99.9],"value":[46807304,48210454,50572656,50572656],"from_merged_histogram":false},"3":{"count":"17","mean":3245245220.2352943,"min":604366238,"max":12811147975,"percentile":[50,90,99,99.9],"value":[2039778659,7385005985,12811147975,12811147975],"from_merged_histogram":false},"4":{"count":"17","mean":14091806.647058824,"min":1184735,"max":43974537,"percentile":[50,90,99,99.9],"value":[7235332,38560594,43974537,43974537],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225628500000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.046692495437956213,"22":317,"23":0,"40":70,"41":84,"42":84,"43":0,"44":14972,"45":14972,"60":32,"64":0.3385692557221562,"66":1},"distributions":{"1":{"count":"21","mean":249826063.80952382,"min":14127820,"max":1045823611,"percentile":[50,90,99,99.9],"value":[50328525,904253176,1045823611,1045823611],"from_merged_histogram":false},"2":{"count":"21","mean":41121581,"min":10386305,"max":49941781,"percentile":[50,90,99,99.9],"value":[47091101,48036959,49941781,49941781],"from_merged_histogram":false},"3":{"count":"21","mean":2908199134.8095236,"min":268258752,"max":8385253565,"percentile":[50,90,99,99.9],"value":[2397514151,5762490481,8385253565,8385253565],"from_merged_histogram":false},"4":{"count":"21","mean":11583122.047619049,"min":391988,"max":43609624,"percentile":[50,90,99,99.9],"value":[4540531,41083746,43609624,43609624],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225628750000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04587992700729925,"22":310,"23":0,"40":70,"41":88,"42":88,"43":0,"44":15768,"45":15704,"60":32,"64":0.3551129391572675,"66":0.9545454545454546},"distributions":{"1":{"count":"22","mean":346967751.3636364,"min":22399023,"max":2438377279,"percentile":[50,90,99,99.9],"value":[72660081,733799796,2438377279,2438377279],"from_merged_histogram":false},"2":{"count":"22","mean":44389885.04545455,"min":10591632,"max":49941781,"percentile":[50,90,99,99.9],"value":[47579526,49098614,49941781,49941781],"from_merged_histogram":false},"3":{"count":"22","mean":3267033925.818182,"min":168143017,"max":13151368425,"percentile":[50,90,99,99.9],"value":[2224213792,5263534134,13151368425,13151368425],"from_merged_histogram":false},"4":{"count":"22","mean":14905903.818181818,"min":1745319,"max":38960647,"percentile":[50,90,99,99.9],"value":[10123934,32337012,38960647,38960647],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225629000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04455720802919709,"22":301,"23":0,"40":70,"41":104,"42":104,"43":0,"44":22408,"45":22408,"60":32,"64":0.36515415704163723,"66":1},"distributions":{"1":{"count":"26","mean":275234055.88461536,"min":19093529,"max":1701681050,"percentile":[50,90,99,99.9],"value":[90035153,870008046,1701681050,1701681050],"from_merged_histogram":false},"2":{"count":"26","mean":40988900.76923077,"min":10374370,"max":48211994,"percentile":[50,90,99,99.9],"value":[47015484,47942301,48211994,48211994],"from_merged_histogram":false},"3":{"count":"26","mean":3330259076.3461537,"min":593588554,"max":10660909453,"percentile":[50,90,99,99.9],"value":[2861559529,7316435354,10660909453,10660909453],"from_merged_histogram":false},"4":{"count":"26","mean":7808944.769230769,"min":876014,"max":27069268,"percentile":[50,90,99,99.9],"value":[4761967,17861315,27069268,27069268],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225629250000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04515130018248174,"22":305,"23":1,"40":70,"41":56,"42":56,"43":0,"44":13036,"45":13036,"60":32,"64":0.3503154686459899,"66":1},"distributions":{"1":{"count":"14","mean":141779998.64285713,"min":23943642,"max":862525029,"percentile":[50,90,99,99.9],"value":[42208157,290020975,862525029,862525029],"from_merged_histogram":false},"2":{"count":"14","mean":42111941.928571425,"min":11486775,"max":48254816,"percentile":[50,90,99,99.9],"value":[46660391,47742749,48254816,48254816],"from_merged_histogram":false},"3":{"count":"14","mean":3391003118,"min":495159080,"max":7585262138,"percentile":[50,90,99,99.9],"value":[2423675626,7487278449,7585262138,7585262138],"from_merged_histogram":false},"4":{"count":"14","mean":7126505.428571428,"min":295740,"max":28424603,"percentile":[50,90,99,99.9],"value":[5887611,13905566,28424603,28424603],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225629500000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04375542883211679,"22":305,"23":1,"40":70,"41":64,"42":64,"43":0,"44":10816,"45":10816,"60":32,"64":0.36122283268085625,"66":1},"distributions":{"1":{"count":"16","mean":444456042.4375,"min":23969869,"max":1571494151,"percentile":[50,90,99,99.9],"value":[109646402,1191319193,1571494151,1571494151],"from_merged_histogram":false},"2":{"count":"16","mean":39458367.4375,"min":10405835,"max":49098614,"percentile":[50,90,99,99.9],"value":[47365904,48211994,49098614,49098614],"from_merged_histogram":false},"3":{"count":"16","mean":2946553721.8125,"min":556717324,"max":7171644080,"percentile":[50,90,99,99.9],"value":[2472973161,5316090996,7171644080,7171644080],"from_merged_histogram":false},"4":{"count":"16","mean":7899679.9375,"min":1982765,"max":29351812,"percentile":[50,90,99,99.9],"value":[5035007,19316448,29351812,29351812],"from_merged_histogram":false}}},"final":false}
{"subscription_id":"export","sim_time_unix_ns":"1767225629750000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.04380031934306569,"22":304,"23":1,"40":70,"41":92,"42":92,"43":0,"44":17112,"45":17112,"60":32,"64":0.33707230772507113,"66":1},"distributions":{"1":{"count":"23","mean":175316606.6956522,"min":20578960,"max":955079286,"percentile":[50,90,99,99.9],"value":[54560172,551176426,955079286,955079286],"from_merged_histogram":false},"2":{"count":"23","mean":43472749.69565217,"min":10524310,"max":50239964,"percentile":[50,90,99,99.9],"value":[47603466,48366904,50239964,50239964],"from_merged_histogram":false},"3":{"count":"23","mean":3298967073,"min":356459242,"max":13181511623,"percentile":[50,90,99,99.9],"value":[2487435555,6910313645,13181511623,13181511623],"from_merged_histogram":false},"4":{"count":"23","mean":7752433.173913044,"min":854572,"max":30051424,"percentile":[50,90,99,99.9],"value":[5577801,17828810,30051424,30051424],"from_merged_histogram":false}}},"final":false}
`;

/**
 * A fleet row as U94b's server emits it: `METRIC_GPU_UTILIZATION` (67, time in step) and
 * `METRIC_GPU_USEFUL_FRACTION` (71, useful work over the maximum possible) each as the fleet mean in `values`
 * and as a distribution across replicas, and `METRIC_KV_UTILIZATION` (20) with the same
 * distribution treatment. Hand-written, so the numbers are round; the excerpt above predates the
 * metrics and stays as exported.
 */
export const GPU_FLEET_ROW = `{"subscription_id":"export","sim_time_unix_ns":"1767225625000000000","realtime_factor":0,"row":{"target":{"scope":"SCOPE_FLEET"},"values":{"20":0.41,"40":70,"41":64,"42":64,"43":0,"60":32,"67":0.62,"68":0.35,"71":0.1},"distributions":{"20":{"count":"32","mean":0.41,"min":0.05,"max":0.97,"percentile":[50,90,99],"value":[0.4,0.8,0.95],"from_merged_histogram":false},"67":{"count":"32","mean":0.62,"min":0.1,"max":0.99,"percentile":[50,90,99],"value":[0.6,0.9,0.98],"from_merged_histogram":false},"71":{"count":"32","mean":0.1,"min":0.01,"max":0.3,"percentile":[50,90,99],"value":[0.08,0.2,0.3],"from_merged_histogram":false}}},"final":false}`;

/** The same run's `scenario.txt`, resolved, so the fake can answer StartRun with a real one. */
export const FLEET_EXCERPT_SCENARIO = `name = p2c
seed = 20260906
duration_s = 120
warmup_s = 15
replicas = 32
max_batch = 256
step_base_ms = 10.2
step_per_seq_ms = 0
step_per_kv_ktoken_ms = 0.0175
kv_capacity_tokens = 1370000
prefill_tokens_per_s = 28286
step_token_budget = 1024
max_queue = 400
disable_decode = false
arrival_rps = 70
prompt_mean = 1200
prompt_cv = 1.2
output_mean = 300
output_cv = 1.5
long_probability = 0.08
long_prompt_mean = 24000
long_output_mean = 400
load_step_at_s = -1
load_step_factor = 1
load_step_until_s = -1
routing = p2c
p2c_choices = 2
probe_live = false
admission = accept_all
admission_headroom = 0.5
fair_share_burst = 2
tenants = 1
tenant_weights = 
tenant_demand = 
telemetry_interval_ms = 1000
telemetry_delay_ms = 200
client_timeout_s = 60
max_attempts = 1
retry_budget_fraction = 1
retry_backoff_s = 0.5
ttft_slo_ms = 2000
itl_slo_ms = 80
e2e_slo_s = 60
sample_interval_ms = 250
`;
