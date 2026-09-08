// The global smoothing window: one selector in the playback bar, every time-series chart reads it.
//
// Issao: "for all the timeseries graph, can you add a global selector of a window average to be
// applied on them, live selectable, to make it easier to smooth out variation/oscilatory patterns?
// We can do client side if that is easier, but ideally it is a metric subscription that we pass
// down to the leaves." So the window is `OpenSubscriptionRequest.smoothing_window_ns` on a live run
// (the server builds every row over the trailing window; WIRE.md "Smoothing") and the same
// trailing-window function over the recorded frames on a replay (`smoothFrames` in adapter.ts).
// This module holds the selection itself: which window, where it is remembered, and how a link
// carries it. Pure functions first, so the self-test can drive them without a window.

import { useSyncExternalStore } from 'react';

export type SmoothingId = 'off' | '1s' | '5s' | '15s' | '30s' | '2m';

export interface SmoothingOption {
  id: SmoothingId;
  /** What the button says. */
  label: string;
  /** Simulated seconds; zero is the raw sample cadence. */
  seconds: number;
}

export const SMOOTHING_OPTIONS: readonly SmoothingOption[] = [
  { id: 'off', label: 'off', seconds: 0 },
  { id: '1s', label: '1 s', seconds: 1 },
  { id: '5s', label: '5 s', seconds: 5 },
  { id: '15s', label: '15 s', seconds: 15 },
  { id: '30s', label: '30 s', seconds: 30 },
  { id: '2m', label: '2 min', seconds: 120 },
];

/** The localStorage key the selection is remembered under, per browser. */
export const SMOOTHING_STORAGE_KEY = 'lbsim.smooth';
/** The query parameter a link carries it in: `?smooth=30s`. */
export const SMOOTHING_QUERY = 'smooth';

export function smoothingSeconds(id: SmoothingId): number {
  return SMOOTHING_OPTIONS.find((o) => o.id === id)?.seconds ?? 0;
}

/** The wire's form: simulated nanoseconds, a bigint like every `uint64`. */
export function smoothingWindowNs(id: SmoothingId): bigint {
  return BigInt(smoothingSeconds(id)) * 1_000_000_000n;
}

/** What a readout beside a chart says about its numbers: "30 s window", or "per sample" when off. */
export function smoothingLabel(id: SmoothingId): string {
  const o = SMOOTHING_OPTIONS.find((x) => x.id === id);
  return !o || o.seconds === 0 ? 'per sample' : `${o.label} window`;
}

/** An id from user-supplied text (a query string, storage); null for anything that is not one. */
export function parseSmoothingId(raw: string | null | undefined): SmoothingId | null {
  if (raw === null || raw === undefined) return null;
  const v = raw.trim().toLowerCase();
  return SMOOTHING_OPTIONS.find((o) => o.id === v)?.id ?? null;
}

/** `?smooth=` from the query string or from a hash route's own query (`#/dashboard?smooth=30s`). */
export function smoothingFromUrl(search: string, hash: string): SmoothingId | null {
  const fromSearch = new URLSearchParams(search).get(SMOOTHING_QUERY);
  if (fromSearch !== null) return parseSmoothingId(fromSearch);
  const q = hash.indexOf('?');
  if (q === -1) return null;
  return parseSmoothingId(new URLSearchParams(hash.slice(q + 1)).get(SMOOTHING_QUERY));
}

/**
 * The query string with the selection written in, or taken out when it is `off`: an absent
 * parameter means "the viewer's own remembered choice", so a plain link never forces raw samples on
 * someone who chose a window. Every other parameter is left exactly as it was.
 */
export function withSmoothingInSearch(search: string, id: SmoothingId): string {
  const params = new URLSearchParams(search);
  if (id === 'off') params.delete(SMOOTHING_QUERY);
  else params.set(SMOOTHING_QUERY, id);
  const s = params.toString();
  return s ? `?${s}` : '';
}

/** The URL wins over storage, storage over the default, so a shared link shows what its author saw. */
export function initialSmoothing(search: string, hash: string, stored: string | null): SmoothingId {
  return smoothingFromUrl(search, hash) ?? parseSmoothingId(stored) ?? 'off';
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

function storedSmoothing(): string | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage.getItem(SMOOTHING_STORAGE_KEY);
  } catch {
    // Storage can be disabled or throw in a private window; that is "nothing stored", not an error.
    return null;
  }
}

let current: SmoothingId =
  typeof window === 'undefined' ? 'off' : initialSmoothing(window.location.search, window.location.hash, storedSmoothing());
const listeners = new Set<() => void>();

export function smoothing(): SmoothingId {
  return current;
}

export function setSmoothing(id: SmoothingId): void {
  if (id === current) return;
  current = id;
  try {
    if (typeof localStorage !== 'undefined') localStorage.setItem(SMOOTHING_STORAGE_KEY, id);
  } catch {
    // Not remembered this time; the selection still applies to this page.
  }
  if (typeof window !== 'undefined' && typeof history !== 'undefined') {
    // The query string, not the hash: the hash is the route, and a route change would remount the
    // page under the selector. `replaceState` leaves the history alone, so back still means back.
    const { pathname, search, hash } = window.location;
    history.replaceState(history.state, '', `${pathname}${withSmoothingInSearch(search, id)}${hash}`);
  }
  for (const l of listeners) l();
}

export function subscribeSmoothing(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/** The selection as a React value; every hook and panel that smooths reads it from here. */
export function useSmoothing(): SmoothingId {
  return useSyncExternalStore(subscribeSmoothing, smoothing, () => 'off');
}
