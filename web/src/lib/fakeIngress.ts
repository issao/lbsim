// An in-memory Ingress server behind a `FetchLike`, speaking `crates/sim-ingress/WIRE.md` exactly.
//
// It exists so the live path of the dashboard can be built and checked before the real server
// answers, and it is deliberately a server and not a stub: the client is driven through
// `IngressClient` and `subscribeToTarget` unchanged, over real `Response` objects and a real
// `ReadableStream` of SSE frames, so what is tested is the contract and not a shortcut past it.
//
// The one thing it does not have is a clock. A real run advances with wall time; this one advances
// when `advance(rows)` or a StepForward says so, which is what makes a self-test deterministic and
// lets it drop a stream at an exact row. Every row it serves is a row of a recorded `fleet.jsonl`,
// so the numbers are the engine's, never invented here.
//
// What it answers, by RPC name: StartRun, StopRun, GetRun, ListRuns, SetSpeed, StepForward,
// OpenSubscription (SSE), RenewSubscription, CloseSubscription, GetResult. Rewind, UpdateWorkload,
// UpdatePolicies and GetTraces answer 501 by default, because the first server answers 501 until
// their units land; `liveUpdates: true` makes the two update calls accept and echo, which is the
// contract they will honour then.

import { rpcPath, type FetchLike, type Overrides } from './api';

export interface FakeFixture {
  /** One SubscriptionUpdate per line, SCOPE_FLEET, as `sim-run export` writes it. */
  fleetJsonl: string;
  /** The run's resolved `scenario.txt`, used when a StartRun sends no text of its own. */
  scenarioText: string;
}

export interface FakeOptions {
  /** How many updates the server keeps for `Last-Event-ID` replay. WIRE.md's first server keeps 256. */
  ring?: number;
  /** A StepForward never advances more than this many rows, whatever it asked for. */
  maxStepRows?: number;
  /** Accept UpdateWorkload and UpdatePolicies instead of answering 501 as the first server does. */
  liveUpdates?: boolean;
}

export interface FakeCall {
  rpc: string;
  /** The JSON body for a POST; the decoded query for the OpenSubscription GET. */
  body: Record<string, unknown>;
  /** The `Last-Event-ID` header, for an OpenSubscription that resumed. */
  lastEventId?: string;
}

export type FakeRunState = 'STATE_RUNNING' | 'STATE_PAUSED' | 'STATE_COMPLETE';

export interface FakeRun {
  runId: string;
  state: FakeRunState;
  /** Rows the run has produced so far; the stream can only send these. Also the highest event id. */
  released: number;
  realtimeFactor: number;
  scenarioText: string;
  overrides: Overrides;
  recordTraces: boolean;
  /** What UpdateWorkload and UpdatePolicies were told, in arrival order. */
  workloadUpdates: Overrides[];
  policyUpdates: Overrides[];
}

export interface FakeIngress extends FetchLike {
  /** Every request seen, in order, so a test can say what the client asked for. */
  readonly calls: FakeCall[];
  runs(): FakeRun[];
  run(runId: string): FakeRun | undefined;
  /** The server's clock: every running run produces `rows` more samples and its streams carry them. */
  advance(rows: number): void;
  /** End every open stream the way a dropped socket would, keeping the subscriptions. Returns how many. */
  dropStreams(): number;
  /** Subscriptions the server still holds. */
  subscriptions(): string[];
  /** Streams currently open. */
  openStreams(): number;
}

interface Row {
  simTimeUnixNs: bigint;
  json: Record<string, unknown>;
}

interface Sub {
  sid: string;
  runId: string;
  /** The highest event id sent on this subscription, i.e. what a resume continues after. */
  lastSent: number;
  ctl: ReadableStreamDefaultController<Uint8Array> | null;
}

const DEFAULT_RING = 256;
const DEFAULT_MAX_STEP_ROWS = 64;
const JSON_HEADERS = { 'content-type': 'application/json' };

function parseRows(text: string): Row[] {
  const rows: Row[] = [];
  for (const line of text.split('\n')) {
    if (line.trim() === '') continue;
    const json = JSON.parse(line) as Record<string, unknown>;
    rows.push({ simTimeUnixNs: BigInt(json.sim_time_unix_ns as string), json });
  }
  if (rows.length < 2) throw new Error('the fake needs at least two rows to know the sample interval');
  return rows;
}

function seedOf(scenarioText: string): string {
  const m = /^\s*seed\s*=\s*(\d+)\s*$/m.exec(scenarioText);
  return m ? m[1] : '0';
}

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: JSON_HEADERS });
}

function error(status: number, message: string): Response {
  return json(status, { error: message });
}

function headerOf(init: RequestInit | undefined, name: string): string | undefined {
  if (!init?.headers) return undefined;
  const v = new Headers(init.headers).get(name);
  return v === null ? undefined : v;
}

function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' ? v : fallback;
}

function num(v: unknown, fallback = 0): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
}

function overrides(v: unknown): Overrides {
  const out: Overrides = {};
  if (v && typeof v === 'object') for (const [k, x] of Object.entries(v as Record<string, unknown>)) out[k] = String(x);
  return out;
}

export function fakeIngress(fixture: FakeFixture, opts: FakeOptions = {}): FakeIngress {
  const rows = parseRows(fixture.fleetJsonl);
  const intervalNs = rows[1].simTimeUnixNs - rows[0].simTimeUnixNs;
  const ring = opts.ring ?? DEFAULT_RING;
  const maxStepRows = opts.maxStepRows ?? DEFAULT_MAX_STEP_ROWS;
  const liveUpdates = opts.liveUpdates ?? false;
  const encoder = new TextEncoder();

  const calls: FakeCall[] = [];
  const runs = new Map<string, FakeRun>();
  const subs = new Map<string, Sub>();
  let nextRun = 1;
  let nextSub = 1;
  // A wall clock that only ever moves forward, for `lease_expires_at_wall_ns`. Never the browser's,
  // and never compared with anything by a correct client, so a counter is enough.
  let wallNs = 1_700_000_000_000_000_000n;
  const wall = (leaseNs: bigint): string => {
    wallNs += 1_000_000n;
    return (wallNs + leaseNs).toString();
  };

  const simTimeOf = (run: FakeRun): bigint => (run.released === 0 ? rows[0].simTimeUnixNs - intervalNs : rows[run.released - 1].simTimeUnixNs);

  const status = (run: FakeRun): Record<string, unknown> => ({
    run_id: run.runId,
    state: run.state,
    sim_time_unix_ns: simTimeOf(run).toString(),
    sim_end_unix_ns: rows[rows.length - 1].simTimeUnixNs.toString(),
    realtime_factor: run.realtimeFactor,
    error: '',
  });

  // -- streams --------------------------------------------------------------

  const send = (sub: Sub, text: string) => {
    sub.ctl?.enqueue(encoder.encode(text));
  };

  const updateEvent = (sub: Sub, id: number, run: FakeRun): string => {
    const row = rows[id - 1];
    const body = { ...row.json, subscription_id: sub.sid, realtime_factor: run.realtimeFactor, final: id === rows.length };
    return `id: ${id}\nevent: update\ndata: ${JSON.stringify(body)}\n\n`;
  };

  /** Everything the run has released and this subscription has not yet been sent. */
  const pump = (sub: Sub) => {
    const run = runs.get(sub.runId);
    if (!run || sub.ctl === null) return;
    while (sub.lastSent < run.released) {
      sub.lastSent++;
      send(sub, updateEvent(sub, sub.lastSent, run));
    }
    if (sub.lastSent === rows.length) {
      // The final update went out; a run that has ended has nothing more to stream.
      sub.ctl.close();
      sub.ctl = null;
    }
  };

  const pumpRun = (runId: string) => {
    for (const sub of subs.values()) if (sub.runId === runId) pump(sub);
  };

  const closeStream = (sub: Sub) => {
    try {
      sub.ctl?.close();
    } catch {
      // Already closed by the reader's cancel; nothing to do.
    }
    sub.ctl = null;
  };

  const openSubscription = (url: URL, init: RequestInit | undefined): Response => {
    const q = Object.fromEntries(url.searchParams.entries());
    const lastEventId = headerOf(init, 'last-event-id');
    calls.push({ rpc: 'OpenSubscription', body: q, lastEventId });

    const run = runs.get(str(q.run_id));
    if (!run) return error(404, `run ${str(q.run_id)} unknown`);
    const leaseNs = BigInt(str(q.lease_ns, '0') || '0');

    const rejected = (why: string): Response => {
      const body = `event: open\ndata: ${JSON.stringify({ subscription_id: '', lease_expires_at_wall_ns: '0', rejected_reason: why })}\n\n`;
      return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
    };
    if (str(q.scope) !== 'SCOPE_FLEET') return rejected(`the fake serves SCOPE_FLEET only, not ${str(q.scope) || '(unset)'}`);

    let sub: Sub;
    const wanted = str(q.subscription_id);
    if (wanted !== '') {
      const known = subs.get(wanted);
      if (!known) return error(410, `subscription ${wanted} is gone`);
      sub = known;
    } else {
      sub = { sid: `s-${nextSub++}`, runId: run.runId, lastSent: 0, ctl: null };
      subs.set(sub.sid, sub);
    }
    // Resume after the id the client last saw; the ring bounds how far back that can be.
    const from = lastEventId === undefined ? sub.lastSent : Number(lastEventId);
    if (!Number.isInteger(from) || from < 0) return error(400, `Last-Event-ID ${lastEventId} is not an event id`);
    if (from < run.released - ring) {
      subs.delete(sub.sid);
      return error(410, `event ${from} is older than the last ${ring} kept`);
    }
    if (sub.ctl !== null) closeStream(sub);
    sub.lastSent = Math.min(from, run.released);

    let mine: ReadableStreamDefaultController<Uint8Array> | null = null;
    const stream = new ReadableStream<Uint8Array>({
      start(ctl) {
        mine = ctl;
        sub.ctl = ctl;
        send(sub, `event: open\ndata: ${JSON.stringify({ subscription_id: sub.sid, lease_expires_at_wall_ns: wall(leaseNs), rejected_reason: '' })}\n\n`);
        pump(sub);
      },
      cancel() {
        if (sub.ctl === mine) sub.ctl = null;
      },
    });
    // An aborted fetch rejects the pending read, which is how the client's `close()` ends a stream.
    init?.signal?.addEventListener('abort', () => {
      if (mine === null || sub.ctl !== mine) return;
      try {
        mine.error(new DOMException('The operation was aborted.', 'AbortError'));
      } catch {
        // Already closed.
      }
      sub.ctl = null;
    });
    return new Response(stream, { status: 200, headers: { 'content-type': 'text/event-stream' } });
  };

  // -- unary RPCs -----------------------------------------------------------

  const advanceRun = (run: FakeRun, n: number) => {
    run.released = Math.min(rows.length, run.released + Math.max(0, n));
    if (run.released === rows.length) run.state = 'STATE_COMPLETE';
    pumpRun(run.runId);
  };

  const findRun = (body: Record<string, unknown>): FakeRun | Response => {
    const run = runs.get(str(body.run_id));
    return run ?? error(404, `run ${str(body.run_id)} unknown`);
  };

  const rpc = (name: string, body: Record<string, unknown>): Response => {
    switch (name) {
      case 'StartRun': {
        const scenario = (body.scenario ?? {}) as Record<string, unknown>;
        const text = str(scenario.text);
        const run: FakeRun = {
          runId: `r-${nextRun++}`,
          state: 'STATE_RUNNING',
          released: 0,
          realtimeFactor: num(body.max_realtime_factor, 0),
          scenarioText: text === '' ? fixture.scenarioText : text,
          overrides: overrides(scenario.overrides),
          recordTraces: body.record_traces === true,
          workloadUpdates: [],
          policyUpdates: [],
        };
        runs.set(run.runId, run);
        return json(200, { run_id: run.runId });
      }
      case 'ListRuns': {
        const limit = num(body.limit, 0);
        const all = [...runs.values()].map(status);
        return json(200, { runs: limit > 0 ? all.slice(0, limit) : all, next_cursor: '' });
      }
      case 'GetRun': {
        const run = findRun(body);
        return run instanceof Response ? run : json(200, status(run));
      }
      case 'StopRun': {
        const run = findRun(body);
        if (run instanceof Response) return run;
        run.state = 'STATE_COMPLETE';
        for (const sub of [...subs.values()]) {
          if (sub.runId !== run.runId) continue;
          closeStream(sub);
          subs.delete(sub.sid);
        }
        return json(200, status(run));
      }
      case 'SetSpeed': {
        const run = findRun(body);
        if (run instanceof Response) return run;
        if (run.state === 'STATE_COMPLETE') return error(409, `run ${run.runId} is complete`);
        run.realtimeFactor = num(body.realtime_factor, run.realtimeFactor);
        run.state = body.paused === true ? 'STATE_PAUSED' : 'STATE_RUNNING';
        return json(200, status(run));
      }
      case 'StepForward': {
        const run = findRun(body);
        if (run instanceof Response) return run;
        if (run.state === 'STATE_COMPLETE') return error(409, `run ${run.runId} is complete`);
        let n: number;
        if (typeof body.sim_duration_ns === 'string') {
          const d = BigInt(body.sim_duration_ns);
          if (d <= 0n) return error(400, 'sim_duration_ns must be positive');
          n = Number((d + intervalNs - 1n) / intervalNs);
        } else if (typeof body.barrier_windows === 'number') {
          n = body.barrier_windows;
        } else {
          return error(400, 'StepForward needs sim_duration_ns or barrier_windows');
        }
        // Stepping pauses at the end of the step: that is what makes it a step rather than a play.
        run.state = 'STATE_PAUSED';
        advanceRun(run, Math.min(n, maxStepRows));
        return json(200, status(run));
      }
      case 'UpdateWorkload':
      case 'UpdatePolicies': {
        if (!liveUpdates) return error(501, `${name} is not supported by the first server`);
        const run = findRun(body);
        if (run instanceof Response) return run;
        const o = overrides(body.overrides);
        (name === 'UpdateWorkload' ? run.workloadUpdates : run.policyUpdates).push(o);
        Object.assign(run.overrides, o);
        // Echoed as accepted, taking effect from the current instant with no resimulation: the fake
        // has no engine to rewind, and the first server applies live changes forward too.
        return json(200, { accepted: true, required_resimulation: false, rewound_to_unix_ns: simTimeOf(run).toString(), rejected_reason: '' });
      }
      case 'RenewSubscription': {
        const sub = subs.get(str(body.subscription_id));
        if (!sub) return json(200, { lease_expires_at_wall_ns: '0', expired: true });
        return json(200, { lease_expires_at_wall_ns: wall(BigInt(str(body.lease_ns, '0') || '0')), expired: false });
      }
      case 'CloseSubscription': {
        const sub = subs.get(str(body.subscription_id));
        if (!sub) return error(404, `subscription ${str(body.subscription_id)} unknown`);
        closeStream(sub);
        subs.delete(sub.sid);
        return json(200, {});
      }
      case 'GetResult': {
        const run = findRun(body);
        if (run instanceof Response) return run;
        if (run.state !== 'STATE_COMPLETE') return error(409, `run ${run.runId} has not finished`);
        // Zero-valued fields omitted, as WIRE.md rule 5 says the server does; the decoder must cope.
        return json(200, { run_id: run.runId, seed: seedOf(run.scenarioText), overall: {}, realtime_factor: run.realtimeFactor });
      }
      case 'Rewind':
      case 'GetTraces':
        return error(501, `${name} is not supported by the first server`);
      default:
        return error(404, `no such rpc: ${name}`);
    }
  };

  // -- the fetch --------------------------------------------------------------

  const prefix = rpcPath('OpenSubscription').slice(0, -'OpenSubscription'.length);

  const fetchImpl = async (input: string, init?: RequestInit): Promise<Response> => {
    const url = new URL(input, 'http://fake.invalid');
    if (!url.pathname.startsWith(prefix)) return error(404, `no such path: ${url.pathname}`);
    const name = url.pathname.slice(prefix.length);
    const method = (init?.method ?? 'GET').toUpperCase();
    if (name === 'OpenSubscription') {
      if (method !== 'GET') return error(405, 'OpenSubscription is a GET');
      return openSubscription(url, init);
    }
    if (method !== 'POST') return error(405, `${name} is a POST`);
    let body: Record<string, unknown> = {};
    if (typeof init?.body === 'string' && init.body !== '') {
      try {
        body = JSON.parse(init.body) as Record<string, unknown>;
      } catch {
        return error(400, 'body is not JSON');
      }
    }
    calls.push({ rpc: name, body });
    return rpc(name, body);
  };

  const fake = fetchImpl as FakeIngress;
  Object.defineProperties(fake, {
    calls: { value: calls },
    runs: { value: () => [...runs.values()] },
    run: { value: (id: string) => runs.get(id) },
    advance: {
      value: (n: number) => {
        for (const run of runs.values()) if (run.state === 'STATE_RUNNING') advanceRun(run, n);
      },
    },
    dropStreams: {
      value: () => {
        let n = 0;
        for (const sub of subs.values()) {
          if (sub.ctl === null) continue;
          closeStream(sub);
          n++;
        }
        return n;
      },
    },
    subscriptions: { value: () => [...subs.keys()] },
    openStreams: { value: () => [...subs.values()].filter((s) => s.ctl !== null).length },
  });
  return fake;
}
