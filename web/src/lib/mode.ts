// Which data the dashboard is showing: a run on the Ingress server (live), or a recording served
// as static files (replay). Live when the server answers, replay when `runs/index.json` is served,
// and when neither is there the page says so instead of drawing anything.
//
// Three ways to point at a server, in precedence order, because they answer different questions.
// `?server=` on the URL points one tab at a server (`?server=off` makes one tab replay while
// others stay live, which is what makes the two comparable side by side). `localStorage['lbsim.server']`
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
 * U70: one vocabulary, the same two words everywhere a data source is named, each carrying its
 * own one-line gloss so a reader never has to guess what the word means on first encounter.
 */
export const DATA_SOURCE_GLOSS = {
  replay: 'a recording of a real engine run',
  live: 'a simulation running on the server now',
} as const;

export const SERVER_BANNER = `live — ${DATA_SOURCE_GLOSS.live}`;

/** The localStorage key. `'1'` means same origin; a URL means that base; `'0'` turns the server off. */
export const STORAGE_KEY = 'lbsim.server';

/** Values that mean "on, at the same origin". */
const TRUTHY = new Set(['1', 'true', 'yes', 'on']);
/** Values that mean "off": replay instead of the server, whatever else is configured. */
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

/** Pure, so the self-test can exercise every precedence without a window. */
export function serverModeFrom(env: string | undefined, search: string, hash: string, stored: string | null = null): ServerMode {
  const q = queryParam('server', search, hash);
  if (q !== null) return decide(q, 'query');
  if (stored !== null && stored.trim() !== '') return decide(stored, 'storage');
  if (env !== undefined && env.trim() !== '') return decide(env, 'env');
  return { enabled: false, baseUrl: '', source: 'default' };
}

function storedFlag(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(STORAGE_KEY);
  } catch {
    // Storage can be disabled or throw in a private window; that is "nothing stored", not an error.
    return null;
  }
}

export function serverMode(): ServerMode {
  const env = typeof import.meta !== 'undefined' ? (import.meta.env?.VITE_LBSIM_SERVER as string | undefined) : undefined;
  if (typeof window === 'undefined') return serverModeFrom(env, '', '', null);
  return serverModeFrom(env, window.location.search, window.location.hash, storedFlag());
}

/** What a live run's banner says: where the server is, when it is not the same origin. */
export function modeBanner(m: ServerMode = serverMode()): string {
  return m.baseUrl === '' ? SERVER_BANNER : `${SERVER_BANNER} at ${m.baseUrl}`;
}

// ---------------------------------------------------------------------------
// Replay: recorded runs served as static files
// ---------------------------------------------------------------------------
//
// When no server answers and `runs/index.json` is served beside the app (what `sim-run export`
// writes and the static server serves), the dashboard plays those recorded runs. Nothing is
// configured for it: an export placed under `web/public/runs/` for local dev, or under the
// container's `--dir` in production, is the switch.

export type DataMode = 'server' | 'replay';

export const REPLAY_BANNER = `replay — ${DATA_SOURCE_GLOSS.replay}`;

/** The word each `DataMode` is called in the UI. The internal name `server` stays; its word is `live`. */
export const DATA_SOURCE_LABEL: Record<DataMode, keyof typeof DATA_SOURCE_GLOSS> = {
  server: 'live',
  replay: 'replay',
};

/** The one-line gloss for a data mode, via its word. Non-empty for every mode, always. */
export function dataSourceGloss(mode: DataMode): string {
  return DATA_SOURCE_GLOSS[DATA_SOURCE_LABEL[mode]];
}

/**
 * The `?replay=` parameter, from the query string or a hash route's own query: `false` skips the
 * recordings for this tab, `true` asks for replay ahead of a server (still subject to the index
 * being there), `null` when absent.
 */
export function replayOverride(search: string, hash: string): boolean | null {
  const raw = queryParam('replay', search, hash);
  if (raw === null) return null;
  const v = raw.trim().toLowerCase();
  if (FALSY.has(v)) return false;
  return true;
}

/** A query parameter from the query string or from a hash route's own query (`#/dashboard?x=`): a link may carry either. */
function queryParam(name: string, search: string, hash: string): string | null {
  const fromSearch = new URLSearchParams(search).get(name);
  if (fromSearch !== null) return fromSearch;
  const q = hash.indexOf('?');
  if (q === -1) return null;
  return new URLSearchParams(hash.slice(q + 1)).get(name);
}

/** How long `ListRuns` gets to answer before the dashboard falls back to replay. */
export const SERVER_PROBE_MS = 1500;

/**
 * Pure precedence: a server that answers wins unless `?replay=` says otherwise; then replay when
 * the index is reachable and not overridden off; then nothing, which the page says in words.
 * Reachability is the caller's to establish, because it is a fetch. `serverReachable` is the
 * probe's answer; a caller with no probe falls back to the flag alone.
 */
export function dataModeFrom(server: ServerMode, indexReachable: boolean, override: boolean | null, serverReachable?: boolean): DataMode | 'none' {
  if ((serverReachable ?? server.enabled) && override === null) return 'server';
  if (indexReachable && override !== false) return 'replay';
  return 'none';
}

/**
 * Does an Ingress server answer `ListRuns` at this base within the budget? False on any refusal,
 * any timeout, and any body that is not a `ListRunsResponse`: a dev server answers every POST with
 * its HTML shell and HTTP 200, and that is not a server. Never probed when the mode was turned off
 * by hand, so `?server=off` still means what it says.
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
    case 'connecting':
      return 'connecting…';
    case 'refused':
      return `live — refused: ${state.detail ?? 'the server refused this run'}`;
    case 'server':
      return state.runId ? `${SERVER_BANNER} · run ${state.runId}` : SERVER_BANNER;
    case 'replay':
      return state.runId ? `${REPLAY_BANNER}: ${state.runId}` : REPLAY_BANNER;
    case 'none':
    default:
      return '';
  }
}

/** One short, accurate sentence per mode, built from the same gloss the badge text uses. */
export function badgeTitle(state: ActiveModeState): string {
  switch (state.mode) {
    case 'connecting':
      return 'Probing for a server or a recorded run; nothing on this page is live yet.';
    case 'refused':
      return `The server refused to start this run: ${state.detail ?? 'unknown error'}.`;
    case 'server':
      return `Every panel here is ${DATA_SOURCE_GLOSS.live}${state.runId ? `, run ${state.runId}` : ''}.`;
    case 'replay':
      return `Every panel here is ${DATA_SOURCE_GLOSS.replay}${state.runId ? `: ${state.runId}` : ''}.`;
    case 'none':
    default:
      return '';
  }
}
