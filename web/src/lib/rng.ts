// Deterministic pseudo-randomness. Everything the mock draws is a pure function of
// (seed, coordinates), so two runs of the same scenario produce identical charts. The real engine
// makes the same promise via named independent random streams; the A/B view depends on it.

/** 32-bit integer hash of an arbitrary number of coordinates. */
export function hash(...coords: number[]): number {
  let h = 0x811c9dc5;
  for (const c of coords) {
    let x = Math.imul(c | 0, 0x9e3779b1) ^ Math.imul(Math.round((c % 1) * 65536), 0x85ebca6b);
    x = (x ^ (x >>> 15)) | 0;
    h = Math.imul(h ^ x, 0x27220a95);
    h = (h ^ (h >>> 13)) | 0;
  }
  return h >>> 0;
}

/** Uniform in [0,1). */
export function uniform(...coords: number[]): number {
  return hash(...coords) / 4294967296;
}

/** Standard normal, Box-Muller, deterministic in the coordinates. */
export function normal(...coords: number[]): number {
  const u1 = Math.max(uniform(...coords, 1), 1e-9);
  const u2 = uniform(...coords, 2);
  return Math.sqrt(-2 * Math.log(u1)) * Math.cos(2 * Math.PI * u2);
}

/** Smooth value noise in one dimension, for drifting signals that do not look jittery. */
export function drift(scale: number, t: number, ...coords: number[]): number {
  const x = t / scale;
  const i = Math.floor(x);
  const f = x - i;
  const s = f * f * (3 - 2 * f);
  const a = normal(i, ...coords);
  const b = normal(i + 1, ...coords);
  return a + (b - a) * s;
}

/** Quantile of a lognormal with the given mean and coefficient of variation. */
export function lognormalQuantile(mean: number, cv: number, p: number): number {
  const sigma2 = Math.log(1 + cv * cv);
  const sigma = Math.sqrt(sigma2);
  const mu = Math.log(Math.max(mean, 1e-9)) - sigma2 / 2;
  return Math.exp(mu + sigma * probit(p));
}

/** Inverse standard normal CDF, Acklam's rational approximation. */
export function probit(p: number): number {
  const q = Math.min(Math.max(p, 1e-9), 1 - 1e-9);
  const a = [-3.969683028665376e1, 2.209460984245205e2, -2.759285104469687e2, 1.38357751867269e2, -3.066479806614716e1, 2.506628277459239];
  const b = [-5.447609879822406e1, 1.615858368580409e2, -1.556989798598866e2, 6.680131188771972e1, -1.328068155288572e1];
  const c = [-7.784894002430293e-3, -3.223964580411365e-1, -2.400758277161838, -2.549732539343734, 4.374664141464968, 2.938163982698783];
  const d = [7.784695709041462e-3, 3.224671290700398e-1, 2.445134137142996, 3.754408661907416];
  const pl = 0.02425;
  if (q < pl) {
    const u = Math.sqrt(-2 * Math.log(q));
    return (((((c[0] * u + c[1]) * u + c[2]) * u + c[3]) * u + c[4]) * u + c[5]) / ((((d[0] * u + d[1]) * u + d[2]) * u + d[3]) * u + 1);
  }
  if (q > 1 - pl) {
    const u = Math.sqrt(-2 * Math.log(1 - q));
    return -(((((c[0] * u + c[1]) * u + c[2]) * u + c[3]) * u + c[4]) * u + c[5]) / ((((d[0] * u + d[1]) * u + d[2]) * u + d[3]) * u + 1);
  }
  const u = q - 0.5;
  const r = u * u;
  return (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * u /
    (((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1);
}

export function clamp(x: number, lo: number, hi: number): number {
  return x < lo ? lo : x > hi ? hi : x;
}

export function mean(xs: ArrayLike<number>): number {
  let s = 0;
  for (let i = 0; i < xs.length; i++) s += xs[i];
  return xs.length ? s / xs.length : 0;
}

/** Coefficient of variation: the direct measure of whether the load balancer is doing its job. */
export function coeffOfVariation(xs: ArrayLike<number>): number {
  const m = mean(xs);
  if (m <= 1e-9) return 0;
  let v = 0;
  for (let i = 0; i < xs.length; i++) v += (xs[i] - m) * (xs[i] - m);
  return Math.sqrt(v / xs.length) / m;
}
