import { useEffect, useSyncExternalStore } from 'react';
import type { MetricId, Target } from './types';
import { RENEW_MS, registry, type RegistryStats } from './subscriptions';

/**
 * Opens one subscription per target for as long as the component is mounted and the document is
 * visible. Unmounting closes them, which is what makes tab switching and pagination bounded.
 */
export function useSubscriptions(
  owner: string,
  targets: Target[],
  metrics: MetricId[],
  samplesPerSimSecond: number
): void {
  // Keyed on the *set* of entities, not the order they are drawn in: re-sorting a page that shows
  // the same twenty replicas must not close and reopen twenty subscriptions.
  const key = JSON.stringify(
    [...targets].sort((a, b) => (a.scope === b.scope ? (a.id ?? -1) - (b.id ?? -1) : a.scope.localeCompare(b.scope)))
  );
  useEffect(() => {
    const ids = (JSON.parse(key) as Target[]).map(
      (t) => registry.open({ target: t, metrics, samplesPerSimSecond, owner }).id
    );
    const renew = window.setInterval(() => {
      if (!registry.renewing) return;
      for (const id of ids) registry.renew(id);
    }, RENEW_MS);
    return () => {
      window.clearInterval(renew);
      for (const id of ids) registry.close(id);
    };
    // metrics and rate are stable per owner in this app; the key covers the target set.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [owner, key, samplesPerSimSecond]);
}

export function useRegistryStats(): RegistryStats {
  return useSyncExternalStore(
    (fn) => registry.subscribe(fn),
    () => cached(),
    () => cached()
  );
}

let cache: RegistryStats | null = null;
let cacheKey = '';
function cached(): RegistryStats {
  const s = registry.stats();
  const k = JSON.stringify(s);
  if (k !== cacheKey) {
    cacheKey = k;
    cache = s;
  }
  return cache!;
}

/** Wires the page visibility API and the lease reaper once, at the app root. */
export function useLeaseLifecycle(): void {
  useEffect(() => {
    const onVis = () => {
      registry.renewing = document.visibilityState === 'visible';
      if (registry.renewing) for (const s of registry.list()) registry.renew(s.id);
    };
    document.addEventListener('visibilitychange', onVis);
    const reap = window.setInterval(() => registry.reap(), 2000);
    onVis();
    return () => {
      document.removeEventListener('visibilitychange', onVis);
      window.clearInterval(reap);
    };
  }, []);
}
