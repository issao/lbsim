// Which data the dashboard is showing: frames generated in this browser, or the Ingress server's.
//
// Mock stays the default, and the mock markers stay with it. Mock is a data source, not a stand-in
// build: this is the product, and mock lets its layout be judged before anything is wired, so a
// build that quietly pointed at a server that is not running would look like a broken dashboard
// rather than an absent one. In mock mode nothing in api.ts or useServerRun.ts executes.
//
// Three ways to turn the server on, in precedence order, because they answer different questions.
// `?server=` on the URL points one tab at a server while every other tab keeps showing mock data,
// which is what makes the two comparable side by side during the wiring. `localStorage['lbsim.server']`
// makes the choice stick for one browser without editing every link. `VITE_LBSIM_SERVER` at build
// time is how a deployed container is pointed at its Ingress. See web/README.md, "Server mode".

import type { IngressClient } from './api';

export interface ServerMode {
  enabled: boolean;
  /** Base URL with no trailing slash. Empty means same origin, which is what a relative fetch wants. */
  baseUrl: string;
  /** Where the decision came from, so the banner can say it rather than the reader guessing. */
  source: 'default' | 'env' | 'storage' | 'query';
}

/**
 * U70: one vocabulary, the same three words everywhere a data source is named, each carrying its
 * own one-line gloss so a reader never has to guess what the word means on first encounter.
 */
export const DATA_SOURCE_GLOSS = {
  mock: 'browser-generated, invented numbers',
  replay: 'a recording of a real engine run',
  live: 'a simulation running on the server now',
} as const;

/** The header marker the stand-in has carried since it was built, now carrying its gloss too. */
export const MOCK_BANNER = `mock — ${DATA_SOURCE_GLOSS.mock}`;
export const SERVER_BANNER = `live — ${DATA_SOURCE_GLOSS.live}`;

/** The localStorage key. `'1'` means same origin; a URL means that base; `'0'` forces mock. */
export const STORAGE_KEY = 'lbsim.server';

/** Values that mean "on, at the same origin". */
const TRUTHY = new Set(['1', 'true', 'yes', 'on']);
/** Values that mean "off": an explicit override back to mock data. */
const FALSY = new Set(['0', 'false', 'no', 'off']);

function normalise(raw: string): string {
  const v = raw.trim();
  if (v === '' || TRUTHY.has(v.toLowerCase())) return '';
  return v.replace(/\/+$/, '');
}

function decide(raw: string, source: ServerMode['source']): ServerMode {
  if (FALSY.has(raw.trim().toLowerCase())) return { enabled: false, baseUrl: '', source };
  return { enabled: true, baseUrl: normalise(raw), source };
}

/**
 * The `?server=` parameter, from the query string or from a hash route's own query. Both, because
 * the app routes on the hash (`#/dashboard`), so a link a reader is handed may carry either.
 */
function queryOverride(search: string, hash: string): string | null {
  const fromSearch = new URLSearchParams(search).get('server');
  if (fromSearch !== null) return fromSearch;
  const q = hash.indexOf('?');
  if (q === -1) return null;
  return new URLSearchParams(hash.slice(q + 1)).get('server');
}

/** Pure, so the self-test can exercise every precedence without a window. */
export function serverModeFrom(env: string | undefined, search: string, hash: string, stored: string | null = null): ServerMode {
  const q = queryOverride(search, hash);
  if (q !== null) return decide(q, 'query');
  if (stored !== null && stored.trim() !== '') return decide(stored, 'storage');
  if (env !== undefined && env.trim() !== '') return decide(env, 'env');
  return { enabled: false, baseUrl: '', source: 'default' };
}

function storedFlag(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage can be disabled or throw in a private window; that is mock mode, not an error.
    return null;
  }
}

export function serverMode(): ServerMode {
  const env = typeof import.meta !== 'undefined' ? (import.meta.env?.VITE_LBSIM_SERVER as string | undefined) : undefined;
  if (typeof window === 'undefined') return serverModeFrom(env, '', '', null);
  return serverModeFrom(env, window.location.search, window.location.hash, storedFlag());
}

/** What the header should say. The mock wording is the existing one, so nothing is weakened. */
export function modeBanner(m: ServerMode = serverMode()): string {
  if (!m.enabled) return MOCK_BANNER;
  return m.baseUrl === '' ? SERVER_BANNER : `${SERVER_BANNER} at ${m.baseUrl}`;
}

// ---------------------------------------------------------------------------
// Replay: recorded runs served as static files
// ---------------------------------------------------------------------------
//
// A third source between the two above. When no server is configured and `runs/index.json` is
// served beside the app (what `sim-run export` writes and the static server serves), the dashboard
// plays those recorded runs. Nothing is configured for it: an export placed under `web/public/runs/`
// for local dev, or under the container's `--dir` in production, is the switch. `?replay=0` forces
// mock for one tab so the two can still be compared side by side. Mock remains the fallback, and
// with no index present nothing about the mock path changes.

export type DataMode = 'mock' | 'server' | 'replay';

export const REPLAY_BANNER = `replay — ${DATA_SOURCE_GLOSS.replay}`;

/** The word each `DataMode` is called in the UI. The internal name `server` stays; its word is `live`. */
export const DATA_SOURCE_LABEL: Record<DataMode, keyof typeof DATA_SOURCE_GLOSS> = {
  mock: 'mock',
  server: 'live',
  replay: 'replay',
};

/** The one-line gloss for a data mode, via its word. Non-empty for every mode, always. */
export function dataSourceGloss(mode: DataMode): string {
  return DATA_SOURCE_GLOSS[DATA_SOURCE_LABEL[mode]];
}

/**
 * The `?replay=` parameter, from the query string or a hash route's own query: `false` forces
 * mock, `true` asks for replay (still subject to the index being there), `null` when absent.
 */
export function replayOverride(search: string, hash: string): boolean | null {
  const raw = queryParam('replay', search, hash);
  if (raw === null) return null;
  const v = raw.trim().toLowerCase();
  if (FALSY.has(v)) return false;
  return true;
}

function queryParam(name: string, search: string, hash: string): string | null {
  const fromSearch = new URLSearchParams(search).get(name);
  if (fromSearch !== null) return fromSearch;
  const q = hash.indexOf('?');
  if (q === -1) return null;
  return new URLSearchParams(hash.slice(q + 1)).get(name);
}

/** How long `ListRuns` gets to answer before the dashboard falls back to replay or mock. */
export const SERVER_PROBE_MS = 1500;

/**
 * Pure precedence: a server that answers wins unless `?replay=` says otherwise; then replay when
 * the index is reachable and not overridden off; then mock. Reachability is the caller's to
 * establish, because it is a fetch. `serverReachable` is the probe's answer; a caller with no probe
 * falls back to the flag alone, which is what the configured-server case meant before the probe
 * existed.
 */
export function dataModeFrom(server: ServerMode, indexReachable: boolean, override: boolean | null, serverReachable?: boolean): DataMode {
  if ((serverReachable ?? server.enabled) && override === null) return 'server';
  if (indexReachable && override !== false) return 'replay';
  return 'mock';
}

/**
 * Does an Ingress server answer `ListRuns` at this base within the budget? False on any refusal,
 * any timeout, and any body that is not a `ListRunsResponse`: a dev server answers every POST with
 * its HTML shell and HTTP 200, and that is mock mode, not a server. Never probed when the mode was
 * turned off by hand, so `?server=0` still means what it says.
 */
export async function probeServer(client: IngressClient, m: ServerMode, timeoutMs = SERVER_PROBE_MS): Promise<boolean> {
  if (!m.enabled && m.source !== 'default') return false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const budget = new Promise<false>((r) => {
    timer = setTimeout(() => r(false), timeoutMs);
  });
  const ctl = new AbortController();
  try {
    return await Promise.race([client.listRuns({ limit: 1 }, ctl.signal).then(() => true), budget]);
  } catch {
    return false;
  } finally {
    if (timer !== null) clearTimeout(timer);
    ctl.abort();
  }
}

export function dataModeBanner(mode: DataMode, server: ServerMode = serverMode(), runId?: string): string {
  if (mode === 'server') return modeBanner(server);
  if (mode === 'replay') return runId ? `${REPLAY_BANNER}: ${runId}` : REPLAY_BANNER;
  return MOCK_BANNER;
}

// The mode a surface has actually resolved to, published so the header badge can follow it. The
// probe is asynchronous and lives in the dashboard; a store is smaller than threading it through
// the router. `none` until a route says otherwise, so the badge never shows a stale claim left
// over from the previous page.
export type ActiveModeName = DataMode | 'none' | 'connecting' | 'refused';

export interface ActiveModeState {
  mode: ActiveModeName;
  runId?: string;
  /** Free text for a state that has one to show, e.g. the server's refusal. */
  detail?: string;
}

let active: ActiveModeState = { mode: 'none' };
const listeners = new Set<() => void>();

export function activeMode(): ActiveModeState {
  return active;
}

export function setActiveMode(mode: ActiveModeName, runId?: string, detail?: string): void {
  if (active.mode === mode && active.runId === runId && active.detail === detail) return;
  active = { mode, runId, detail };
  for (const l of listeners) l();
}

export function subscribeActiveMode(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/**
 * What the badge says. Names what is on screen right now, not what the build can do overall:
 * `none` and `connecting` both render nothing wrong rather than a stale claim from the last page.
 */
export function badgeText(state: ActiveModeState): string {
  switch (state.mode) {
    case 'none':
      return '';
    case 'connecting':
      return 'connecting…';
    case 'refused':
      return `live — refused: ${state.detail ?? 'the server refused this run'}`;
    case 'server':
      return state.runId ? `${SERVER_BANNER} · run ${state.runId}` : SERVER_BANNER;
    case 'replay':
      return state.runId ? `${REPLAY_BANNER}: ${state.runId}` : REPLAY_BANNER;
    case 'mock':
    default:
      return MOCK_BANNER;
  }
}

/** One short, accurate sentence per mode, built from the same gloss the badge text uses. */
export function badgeTitle(state: ActiveModeState): string {
  switch (state.mode) {
    case 'none':
      return '';
    case 'connecting':
      return 'Probing for a server or a recorded run; nothing on this page is live yet.';
    case 'refused':
      return `The server refused to start this run: ${state.detail ?? 'unknown error'}.`;
    case 'server':
      return `Every panel here is ${DATA_SOURCE_GLOSS.live}${state.runId ? `, run ${state.runId}` : ''}.`;
    case 'replay':
      return `Every panel here is ${DATA_SOURCE_GLOSS.replay}${state.runId ? `: ${state.runId}` : ''}.`;
    case 'mock':
    default:
      return `Every panel here is ${DATA_SOURCE_GLOSS.mock}.`;
  }
}
