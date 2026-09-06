// A mock of the subscription lifecycle from subscription.proto, kept because the pagination story
// in docs/ui-spec.md section 2 is one of the things this stand-in exists to prove or disprove.
//
// Nothing streams here: the frames are already in memory. What is real is the bookkeeping. One
// subscription names exactly one entity, the machine-level view opens twenty of them for the rows
// it is showing and closes them when the page changes, switching tabs closes the previous tab's,
// and a hidden document stops renewing leases so they expire. The readout in the status bar shows
// all of it, which is the only way to tell whether the design is actually bounded.

import type { MetricId, Target } from './types';

export const LEASE_MS = 30_000;
export const RENEW_MS = 10_000;

export interface Subscription {
  id: string;
  target: Target;
  metrics: MetricId[];
  samplesPerSimSecond: number;
  openedAt: number;
  leaseExpiresAt: number;
  renewals: number;
  owner: string;
}

export interface RegistryStats {
  open: number;
  peak: number;
  opened: number;
  closed: number;
  expired: number;
  renewals: number;
  renewing: boolean;
  byOwner: Record<string, number>;
}

type Listener = () => void;

export class SubscriptionRegistry {
  private subs = new Map<string, Subscription>();
  private listeners = new Set<Listener>();
  private nextId = 1;
  private counters = { opened: 0, closed: 0, expired: 0, renewals: 0, peak: 0 };
  renewing = true;

  open(req: { target: Target; metrics: MetricId[]; samplesPerSimSecond: number; owner: string }): Subscription {
    const id = `sub-${this.nextId++}`;
    const now = Date.now();
    const sub: Subscription = {
      id,
      target: req.target,
      metrics: req.metrics,
      samplesPerSimSecond: req.samplesPerSimSecond,
      openedAt: now,
      leaseExpiresAt: now + LEASE_MS,
      renewals: 0,
      owner: req.owner,
    };
    this.subs.set(id, sub);
    this.counters.opened++;
    this.counters.peak = Math.max(this.counters.peak, this.subs.size);
    this.emit();
    return sub;
  }

  renew(id: string): boolean {
    const s = this.subs.get(id);
    if (!s) return false;
    s.leaseExpiresAt = Date.now() + LEASE_MS;
    s.renewals++;
    this.counters.renewals++;
    this.emit();
    return true;
  }

  close(id: string): void {
    if (this.subs.delete(id)) {
      this.counters.closed++;
      this.emit();
    }
  }

  /** The server drops what nobody renewed. Called on a timer so the readout is honest. */
  reap(): void {
    const now = Date.now();
    let n = 0;
    for (const [id, s] of this.subs) if (s.leaseExpiresAt < now) { this.subs.delete(id); n++; }
    if (n > 0) {
      this.counters.expired += n;
      this.emit();
    }
  }

  list(): Subscription[] {
    return [...this.subs.values()];
  }

  stats(): RegistryStats {
    const byOwner: Record<string, number> = {};
    for (const s of this.subs.values()) byOwner[s.owner] = (byOwner[s.owner] ?? 0) + 1;
    return { open: this.subs.size, ...this.counters, renewing: this.renewing, byOwner };
  }

  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    for (const fn of this.listeners) fn();
  }
}

export const registry = new SubscriptionRegistry();
