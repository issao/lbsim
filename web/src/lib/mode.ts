// Which data the dashboard is showing: frames generated in this browser, or the Ingress server's.
//
// Mock stays the default, and the mock markers stay with it. The stand-in exists so the layout can
// be criticised before anything is wired, and a build that quietly pointed at a server that is not
// running would look like a broken dashboard rather than an absent one.
//
// Two ways to turn the server on, because they answer different questions. `VITE_LBSIM_SERVER` at
// build time is how a deployed container is pointed at its Ingress. `?server=1` on the URL is how
// one tab is pointed at a server while every other tab keeps showing mock data, which is what makes
// the two comparable side by side during the wiring.

export interface ServerMode {
  enabled: boolean;
  /** Base URL with no trailing slash. Empty means same origin, which is what a relative fetch wants. */
  baseUrl: string;
  /** Where the decision came from, so the banner can say it rather than the reader guessing. */
  source: 'default' | 'env' | 'query';
}

/** The header marker the stand-in has carried since it was built. Unchanged on purpose. */
export const MOCK_BANNER = 'mock data, no engine attached';
export const SERVER_BANNER = 'live data from the Ingress server';

/** Values of `VITE_LBSIM_SERVER` or `?server=` that mean "on, at the same origin". */
const TRUTHY = new Set(['1', 'true', 'yes', 'on']);

function normalise(raw: string): string {
  const v = raw.trim();
  if (v === '' || TRUTHY.has(v.toLowerCase())) return '';
  return v.replace(/\/+$/, '');
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

export function serverModeFrom(env: string | undefined, search: string, hash: string): ServerMode {
  const q = queryOverride(search, hash);
  if (q !== null) {
    // An explicit `?server=0` is a way to force mock data on a build that defaults to a server.
    if (q === '0' || q.toLowerCase() === 'false') return { enabled: false, baseUrl: '', source: 'query' };
    return { enabled: true, baseUrl: normalise(q), source: 'query' };
  }
  if (env !== undefined && env !== '' && env !== '0' && env.toLowerCase() !== 'false') {
    return { enabled: true, baseUrl: normalise(env), source: 'env' };
  }
  return { enabled: false, baseUrl: '', source: 'default' };
}

export function serverMode(): ServerMode {
  const env = typeof import.meta !== 'undefined' ? (import.meta.env?.VITE_LBSIM_SERVER as string | undefined) : undefined;
  if (typeof window === 'undefined') return serverModeFrom(env, '', '');
  return serverModeFrom(env, window.location.search, window.location.hash);
}

/** What the header should say. The mock wording is the existing one, so nothing is weakened. */
export function modeBanner(m: ServerMode = serverMode()): string {
  if (!m.enabled) return MOCK_BANNER;
  return m.baseUrl === '' ? SERVER_BANNER : `${SERVER_BANNER} at ${m.baseUrl}`;
}
