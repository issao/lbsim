#!/usr/bin/env python3
"""Empirical Bode plot of the staleness loop (M7, U33). Standard library only.

Reads every ``out/18-bode/*.series.csv`` that demo 18 wrote, takes ``offered_rps`` as the input and
``fleet_running`` (requests in flight across the fleet) as the output, and at each run's perturbation
frequency measures the output's gain and phase relative to the input by correlation with sin and cos
over an integer number of cycles. ``fleet_running`` rather than ``fleet_queue`` because at this fleet
an arrival is admitted straight into a batch (max_batch 256 against ~23 running per replica), so the
backlog the herd builds lives in the running count; ``fleet_queue`` averages 1.7 requests and tracks
the arrival rate with unity gain and no lag, which says nothing about the loop. The per-replica
spread (standard deviation of load across replicas at each sample) is the herd's own variable, and
its dominant frequency is reported beside the fleet's.
The frequency where the phase lag reaches 180 degrees is where a loop with unity gain oscillates on
its own; a pure delay of ``D + I/2`` (telemetry delay plus half the publication interval, the mean
age of the snapshot least_requests reads) predicts that crossing at ``1 / (2 (D + I/2))``.

Writes ``out/18-bode-plot.html``: an inline-SVG Bode plot, a table, and the prediction.
"""

import csv
import glob
import math
import os
import re
import sys

TELEMETRY_DIR = "out/18-bode"
OUT_HTML = "out/18-bode-plot.html"
SCENARIO = "scenarios/bode.txt"
INPUT = "offered_rps"
OUTPUT = "fleet_running"
FLEET_SERIES = ("offered_rps", "fleet_queue", "fleet_running", "fleet_kv_utilization")
SCAN_LO, SCAN_HI, SCAN_STEP = 0.005, 2.0, 0.005


def scenario_params(path):
    keys = {}
    with open(path) as f:
        for line in f:
            line = line.split("#", 1)[0].strip()
            if "=" in line:
                k, v = (s.strip() for s in line.split("=", 1))
                keys[k] = v
    return keys


def read_series(path):
    """{series name: (t_s, v)} with t in seconds from the earliest sample in the file."""
    by_name = {}
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            by_name.setdefault(row["series"], ([], [])) 
            t, v = by_name[row["series"]]
            t.append(int(row["t_unix_ns"]))
            v.append(float(row["value"]))
    t0 = min(t[0] for t, _ in by_name.values() if t)
    return {name: ([(x - t0) / 1e9 for x in t], v) for name, (t, v) in by_name.items()}


def phasor(t, v, f):
    """Complex amplitude of the component at f: (2/N) * sum v e^{-i 2 pi f t}, mean removed."""
    n = len(v)
    mean = sum(v) / n
    re_ = im_ = 0.0
    for ti, vi in zip(t, v):
        a = 2.0 * math.pi * f * ti
        d = vi - mean
        re_ += d * math.cos(a)
        im_ -= d * math.sin(a)
    return complex(2.0 * re_ / n, 2.0 * im_ / n), mean


def window(t, v, start_s, f):
    """Samples from start_s spanning an integer number of cycles of f (at least one)."""
    end_s = t[-1]
    cycles = max(1, int(math.floor((end_s - start_s) * f)))
    stop = start_s + cycles / f
    keep = [(ti, vi) for ti, vi in zip(t, v) if start_s <= ti < stop]
    return [k[0] for k in keep], [k[1] for k in keep], cycles


def analyse(series, f, warmup_s):
    ti, xi = series[INPUT]
    to, yo = series[OUTPUT]
    # Both series share the sampler's clock, so the same window (and so the same phase reference)
    # applies to each; an integer number of cycles keeps the correlation free of end effects.
    ti, xi, cycles = window(ti, xi, warmup_s, f)
    to, yo, _ = window(to, yo, warmup_s, f)
    xf, xm = phasor(ti, xi, f)
    yf, ym = phasor(to, yo, f)
    in_rel = abs(xf) / xm if xm else float("nan")
    out_rel = abs(yf) / ym if ym else float("nan")
    gain = out_rel / in_rel if in_rel else float("nan")
    phase = math.degrees(math.atan2(yf.imag, yf.real) - math.atan2(xf.imag, xf.real))
    phase = (phase + 180.0) % 360.0 - 180.0

    # Harmonic content: what is left of the output once its fundamental is taken out, relative to
    # that fundamental. Large means the queue is moving at some frequency other than the input's.
    fund_rms = abs(yf) / math.sqrt(2.0)
    resid = 0.0
    for tt, yy in zip(to, yo):
        a = 2.0 * math.pi * f * tt
        fitted = ym + yf.real * math.cos(a) - yf.imag * math.sin(a)
        resid += (yy - fitted) ** 2
    resid_rms = math.sqrt(resid / len(yo))
    thd = resid_rms / fund_rms if fund_rms else float("nan")

    # The output's own dominant frequency, over the whole post-warm-up record.
    to_all, yo_all = series[OUTPUT]
    to_all = [tt for tt in to_all if tt >= warmup_s]
    yo_all = yo_all[len(yo_all) - len(to_all):]
    best_f, best_a = None, -1.0
    k = 0
    while True:
        fs = SCAN_LO + k * SCAN_STEP
        if fs > SCAN_HI + 1e-9:
            break
        amp = abs(phasor(to_all, yo_all, fs)[0])
        if amp > best_a:
            best_f, best_a = fs, amp
        k += 1
    return dict(
        f=f, cycles=cycles, in_mean=xm, in_amp=abs(xf), out_mean=ym, out_amp=abs(yf),
        in_rel=in_rel, out_rel=out_rel, gain=gain,
        gain_db=20.0 * math.log10(gain) if gain > 0 else float("nan"),
        phase=phase, thd=thd, dominant_f=best_f, dominant_amp=best_a,
    )


def replica_spread(series):
    """Standard deviation of load across replicas at each sample: the herd, as a time series."""
    names = [n for n in series if n not in FLEET_SERIES]
    t = series[names[0]][0]
    cols = [series[n][1] for n in names]
    spread = []
    for k in range(len(t)):
        vals = [c[k] for c in cols]
        m = sum(vals) / len(vals)
        spread.append(math.sqrt(sum((v - m) ** 2 for v in vals) / len(vals)))
    return t, spread


def unwrap(rows):
    """Phase lag accumulates with frequency; pick each point's representation nearest the last."""
    prev = 0.0
    for r in rows:
        p = r["phase"]
        while p - prev > 180.0:
            p -= 360.0
        while p - prev < -180.0:
            p += 360.0
        r["phase"] = p
        prev = p


def crossing(rows, level=-180.0):
    """Interpolated frequency (in log f) where the unwrapped phase first passes `level`."""
    for a, b in zip(rows, rows[1:]):
        if a["phase"] > level >= b["phase"]:
            w = (a["phase"] - level) / (a["phase"] - b["phase"])
            return math.exp(math.log(a["f"]) + w * (math.log(b["f"]) - math.log(a["f"])))
    return None


def svg_panel(rows, key, ylabel, f_pred, f_meas, width=640, height=220, ylim=None):
    fs = [r["f"] for r in rows]
    ys = [r[key] for r in rows if not math.isnan(r[key])]
    lx, ly, rx, by = 60, 16, width - 16, height - 36
    lf0, lf1 = math.log10(min(fs)) - 0.15, math.log10(max(fs)) + 0.15
    if f_pred:
        lf1 = max(lf1, math.log10(f_pred) + 0.15)
    if ylim:
        y0, y1 = ylim
    else:
        pad = max(1e-9, (max(ys) - min(ys)) * 0.1)
        y0, y1 = min(ys) - pad, max(ys) + pad

    def X(f):
        return lx + (math.log10(f) - lf0) / (lf1 - lf0) * (rx - lx)

    def Y(v):
        return by - (v - y0) / (y1 - y0) * (by - ly)

    out = [f'<svg viewBox="0 0 {width} {height}" width="{width}" height="{height}" role="img">']
    out.append(f'<rect x="{lx}" y="{ly}" width="{rx-lx}" height="{by-ly}" fill="none" stroke="#999"/>')
    # Decade grid.
    d = math.ceil(lf0)
    while d <= lf1:
        f = 10 ** d
        out.append(f'<line x1="{X(f):.1f}" y1="{ly}" x2="{X(f):.1f}" y2="{by}" stroke="#ddd"/>')
        out.append(f'<text x="{X(f):.1f}" y="{by+14}" font-size="10" text-anchor="middle">{f:g}</text>')
        d += 1
    for tick in range(5):
        v = y0 + (y1 - y0) * tick / 4
        out.append(f'<line x1="{lx}" y1="{Y(v):.1f}" x2="{rx}" y2="{Y(v):.1f}" stroke="#eee"/>')
        out.append(f'<text x="{lx-4}" y="{Y(v)+3:.1f}" font-size="10" text-anchor="end">{v:.0f}</text>')
    if key == "phase":
        out.append(f'<line x1="{lx}" y1="{Y(-180):.1f}" x2="{rx}" y2="{Y(-180):.1f}" stroke="#c44" stroke-dasharray="4 3"/>')
    if f_pred:
        out.append(f'<line x1="{X(f_pred):.1f}" y1="{ly}" x2="{X(f_pred):.1f}" y2="{by}" stroke="#c44" stroke-dasharray="4 3"/>')
        out.append(f'<text x="{X(f_pred)+3:.1f}" y="{ly+11}" font-size="10" fill="#c44">predicted {f_pred:.2f} Hz</text>')
    if f_meas:
        out.append(f'<line x1="{X(f_meas):.1f}" y1="{ly}" x2="{X(f_meas):.1f}" y2="{by}" stroke="#27a" stroke-dasharray="2 3"/>')
        out.append(f'<text x="{X(f_meas)+3:.1f}" y="{ly+24}" font-size="10" fill="#27a">measured {f_meas:.2f} Hz</text>')
    pts = " ".join(f"{X(r['f']):.1f},{Y(r[key]):.1f}" for r in rows if not math.isnan(r[key]))
    out.append(f'<polyline points="{pts}" fill="none" stroke="#27a" stroke-width="1.5"/>')
    for r in rows:
        if not math.isnan(r[key]):
            out.append(f'<circle cx="{X(r["f"]):.1f}" cy="{Y(r[key]):.1f}" r="3" fill="#27a"/>')
    out.append(f'<text x="{(lx+rx)/2:.0f}" y="{height-4}" font-size="11" text-anchor="middle">frequency (Hz, log)</text>')
    out.append(f'<text x="12" y="{(ly+by)/2:.0f}" font-size="11" text-anchor="middle" transform="rotate(-90 12 {(ly+by)/2:.0f})">{ylabel}</text>')
    out.append("</svg>")
    return "\n".join(out)


def main():
    files = sorted(glob.glob(os.path.join(TELEMETRY_DIR, "*.series.csv")))
    if not files:
        sys.exit(f"no {TELEMETRY_DIR}/*.series.csv: run demo 18 first (run-demos.sh)")
    params = scenario_params(SCENARIO)
    D = float(params.get("telemetry_delay_ms", "200")) / 1000.0
    I = float(params.get("telemetry_interval_ms", "1000")) / 1000.0
    warmup_s = float(params.get("warmup_s", "30"))
    amp = float(params.get("perturb_amplitude", "0.3"))
    f_pred = 1.0 / (2.0 * (D + I / 2.0))

    sampling = {}
    manifest = os.path.join(TELEMETRY_DIR, "manifest.csv")
    if os.path.exists(manifest):
        with open(manifest, newline="") as f:
            for row in csv.DictReader(f):
                if row.get("file", "").endswith(".series.csv"):
                    sampling[row["run"]] = row.get("sampling", "?")

    rows = []
    for path in files:
        # The frequency comes from the scenario the telemetry dir wrote beside the series, since
        # the file name spells 0.05 as 0_05.
        run_params = scenario_params(re.sub(r"\.series\.csv$", ".scenario.txt", path))
        if "perturb_frequency_hz" not in run_params:
            print(f"skip {path}: no perturb_frequency_hz in its scenario", file=sys.stderr)
            continue
        f = float(run_params["perturb_frequency_hz"])
        series = read_series(path)
        if INPUT not in series or OUTPUT not in series:
            print(f"skip {path}: missing {INPUT} or {OUTPUT}", file=sys.stderr)
            continue
        r = analyse(series, f, warmup_s)
        spread = analyse({INPUT: series[INPUT], OUTPUT: replica_spread(series)}, f, warmup_s)
        r["spread_rel"] = spread["out_rel"]
        r["spread_thd"] = spread["thd"]
        r["spread_dominant_f"] = spread["dominant_f"]
        r["file"] = os.path.basename(path)
        r["samples"] = len(series[OUTPUT][0])
        rows.append(r)
    rows.sort(key=lambda r: r["f"])
    unwrap(rows)
    f_meas = crossing(rows)
    peak = max(rows, key=lambda r: r["gain"])

    print(f"loop: D = {D:.3f} s, I = {I:.3f} s, D + I/2 = {D + I/2:.3f} s, predicted 180 deg crossing {f_pred:.3f} Hz")
    hdr = (f"{'f Hz':>7} {'cycles':>6} {'in amp':>7} {'out amp':>8} {'gain':>7} {'dB':>7} {'phase':>8} "
           f"{'harm':>6} {'dom Hz':>7} {'spr amp':>8} {'spr harm':>8} {'spr dom':>8}")
    print(hdr)
    for r in rows:
        print(f"{r['f']:7.3f} {r['cycles']:6d} {r['in_rel']:7.3f} {r['out_rel']:8.3f} {r['gain']:7.3f} "
              f"{r['gain_db']:7.2f} {r['phase']:8.1f} {r['thd']:6.2f} {r['dominant_f']:7.3f} "
              f"{r['spread_rel']:8.3f} {r['spread_thd']:8.2f} {r['spread_dominant_f']:8.3f}")
    print(f"measured 180 deg crossing: {f_meas:.3f} Hz" if f_meas else "measured 180 deg crossing: none in the sweep")
    print(f"largest gain at {peak['f']:g} Hz")
    if sampling:
        print("sampling: " + "; ".join(f"{k}: {v}" for k, v in sampling.items()))

    gain_svg = svg_panel(rows, "gain_db", "gain (dB)", f_pred, f_meas)
    phase_svg = svg_panel(rows, "phase", "phase (deg)", f_pred, f_meas,
                          ylim=(min(-200.0, min(r["phase"] for r in rows) - 20), 20.0))
    table = ["<table><thead><tr><th>f (Hz)</th><th>cycles</th><th>input amplitude (rel.)</th>"
             "<th>output amplitude (rel.)</th><th>gain</th><th>gain (dB)</th><th>phase (deg)</th>"
             "<th>harmonic content</th><th>output dominant f (Hz)</th><th>replica spread amplitude (rel.)</th>"
             "<th>spread harmonic content</th><th>spread dominant f (Hz)</th><th>samples</th></tr></thead><tbody>"]
    for r in rows:
        table.append(f"<tr><td>{r['f']:g}</td><td>{r['cycles']}</td><td>{r['in_rel']:.3f}</td>"
                     f"<td>{r['out_rel']:.3f}</td><td>{r['gain']:.3f}</td><td>{r['gain_db']:.2f}</td>"
                     f"<td>{r['phase']:.1f}</td><td>{r['thd']:.2f}</td><td>{r['dominant_f']:.3f}</td>"
                     f"<td>{r['spread_rel']:.3f}</td><td>{r['spread_thd']:.2f}</td><td>{r['spread_dominant_f']:.3f}</td>"
                     f"<td>{r['samples']}</td></tr>")
    table.append("</tbody></table>")
    meas_txt = f"{f_meas:.2f} Hz" if f_meas else "not reached inside the sweep"
    html = f"""<!doctype html>
<meta charset="utf-8">
<title>18. Bode plot of the staleness loop</title>
<style>
body {{ font: 14px/1.4 system-ui, sans-serif; margin: 24px; max-width: 900px; color: #222; }}
table {{ border-collapse: collapse; font-variant-numeric: tabular-nums; }}
th, td {{ border: 1px solid #ccc; padding: 3px 8px; text-align: right; }}
th {{ background: #f3f3f3; }}
figcaption {{ font-size: 13px; color: #555; margin-top: 6px; }}
</style>
<h1>18. The staleness loop's Bode plot</h1>
<p>Input: offered load, a sine of amplitude {amp:.0%} around {rows[0]['in_mean']:.0f} rps.
Output: requests in flight across the fleet (<code>fleet_running</code>, mean {rows[0]['out_mean']:.0f}),
which is where the backlog lives here: an arrival is admitted straight into a batch, so
<code>fleet_queue</code> averages under two requests and only echoes the arrival rate. least_requests over {params.get('replicas', '?')} replicas reads a
snapshot D = {D * 1000:.0f} ms old, refreshed every I = {I * 1000:.0f} ms, so the loop behaves like
a delay of D + I/2 = {D + I / 2:.2f} s.</p>
<figure>{gain_svg}<br>{phase_svg}
<figcaption>Gain is the in-flight count's relative amplitude over the offered load's relative amplitude, at
the input frequency, by correlation over an integer number of cycles after {warmup_s:.0f} s of
warm-up. A pure delay of {D + I / 2:.2f} s lags 360 &middot; f &middot; {D + I / 2:.2f} degrees, so
its 180&deg; crossing is predicted at f<sub>180</sub> = 1 / (2 (D + I/2)) = {f_pred:.2f} Hz
(red). The measured crossing is {meas_txt} (blue).</figcaption></figure>
<p><b>Prediction:</b> f<sub>180</sub> = 1 / (2 (D + I/2)) = {f_pred:.2f} Hz; measured {meas_txt}.
The largest gain is at {peak['f']:g} Hz ({peak['gain_db']:.1f} dB, phase {peak['phase']:.0f}&deg;).</p>
{''.join(table)}
<p>Harmonic content is the RMS of the output minus its fitted fundamental, over the fundamental's
RMS: near zero the fleet follows the input; large means it is moving at a frequency of its own,
listed in the dominant-frequency column (scan {SCAN_LO}&ndash;{SCAN_HI} Hz in {SCAN_STEP} Hz steps).
The replica spread columns apply the same analysis to the standard deviation of load across the
{params.get('replicas', '?')} replicas at each sample, which is the herd itself: its dominant frequency is
the frequency the load sloshes at.
{'Series sampling: ' + '; '.join(f'{k}: {v}' for k, v in sampling.items()) if sampling else ''}</p>
"""
    os.makedirs(os.path.dirname(OUT_HTML), exist_ok=True)
    with open(OUT_HTML, "w") as f:
        f.write(html)
    print(f"wrote {OUT_HTML}")


if __name__ == "__main__":
    main()
