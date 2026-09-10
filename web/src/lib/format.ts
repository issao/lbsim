export function fmtMs(ms: number): string {
  if (!isFinite(ms)) return '-';
  if (ms >= 60000) return `${(ms / 60000).toFixed(1)} min`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(ms >= 10000 ? 0 : 2)} s`;
  if (ms >= 10) return `${ms.toFixed(0)} ms`;
  return `${ms.toFixed(1)} ms`;
}

export function fmtNum(x: number, digits = 1): string {
  if (!isFinite(x)) return '-';
  if (Math.abs(x) >= 1e9) return `${(x / 1e9).toFixed(2)}G`;
  if (Math.abs(x) >= 1e6) return `${(x / 1e6).toFixed(2)}M`;
  if (Math.abs(x) >= 1e4) return `${(x / 1e3).toFixed(1)}k`;
  return x.toFixed(digits);
}

export function fmtPct(x: number, digits = 1): string {
  // A replayed frame carries NaN for a quantity the engine does not simulate; a dash, like fmtNum.
  if (!isFinite(x)) return '-';
  return `${(x * 100).toFixed(digits)}%`;
}

export function fmtTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = s - m * 60;
  return `${m}:${sec < 10 ? '0' : ''}${sec.toFixed(1)}`;
}

/** An absolute simulated instant (unix-ns, per api.ts's convention) as `HH:MM:SS.mmm`, UTC — a
 * label for a hover, never a value read back as data. Safe to go through `Number`: ms-since-epoch
 * for a simulated 2026 instant is ~1.8e12, far under 2^53. */
export function fmtAbsNs(ns: bigint): string {
  const d = new Date(Number(ns / 1_000_000n));
  if (!isFinite(d.getTime())) return '-';
  return d.toISOString().slice(11, 23);
}

export function fmtTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k`;
  return n.toFixed(0);
}
