// Which data the dashboard is showing: frames generated in this browser, or the Ingress server's.
//
// Mock stays the default, and the mock markers stay with it. The stand-in exists so the layout can
// be criticised before anything is wired, and a build that quietly pointed at a server that is not
// running would look like a broken dashboard rather than an absent one. In mock mode nothing in
// api.ts or useServerRun.ts executes.
//
// Three ways to turn the server on, in precedence order, because they answer different questions.
// `?server=` on the URL points one tab at a server while every other tab keeps showing mock data,
// which is what makes the two comparable side by side during the wiring. `localStorage['lbsim.server']`
// makes the choice stick for one browser without editing every link. `VITE_LBSIM_SERVER` at build
// time is how a deployed container is pointed at its Ingress. See web/README.md, "Server mode".

export interface ServerMode {
  enabled: boolean;
  /** Base URL with no trailing slash. Empty means same origin, which is what a relative fetch wants. */
  baseUrl: string;
  /** Where the decision came from, so the banner can say it rather than the reader guessing. */
  source: 'default' | 'env' | 'storage' | 'query';
}

/** The header marker the stand-in has carried since it was built. Unchanged on purpose. */
export const MOCK_BANNER = 'mock data, no engine attached';
export const SERVER_BANNER = 'live data from the Ingress server';

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
