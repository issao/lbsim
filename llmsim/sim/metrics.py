"""Metric collection and summary statistics.

Three kinds of data, because they answer different questions:

- **Events**: one row per occurrence, with arbitrary fields. Request lifecycles
  live here. This is the raw material for latency distributions.
- **Series**: ``(time, value)`` samples of a gauge. Queue depth, KV utilisation,
  replica count. This is what you plot to see a dynamic unfold.
- **Counters**: monotonic totals. Requests dropped, preemptions, retries.

Percentiles are computed with linear interpolation, matching the default of
``numpy.percentile``, so numbers here are comparable with analyses done
elsewhere. No numpy dependency: this environment has no package manager.
"""

from __future__ import annotations

import csv
import json
import math
from collections import defaultdict
from dataclasses import dataclass, field
from typing import Any, Iterable, Sequence


def percentile(values: Sequence[float], q: float) -> float:
    """``q`` in [0, 100]. Linear interpolation between order statistics."""
    if not values:
        return math.nan
    ordered = sorted(values)
    if len(ordered) == 1:
        return float(ordered[0])
    pos = (len(ordered) - 1) * (q / 100.0)
    lo = math.floor(pos)
    hi = math.ceil(pos)
    if lo == hi:
        return float(ordered[int(pos)])
    frac = pos - lo
    return float(ordered[lo] + (ordered[hi] - ordered[lo]) * frac)


def summarize(values: Sequence[float], percentiles: Sequence[float] = (50, 90, 95, 99, 99.9)) -> dict[str, float]:
    """Count, mean, min, max and the requested percentiles.

    Tail percentiles are the point of this simulator, so p99 and p99.9 are in
    the default set. Note that p99.9 needs on the order of 10k samples to mean
    anything; ``count`` is reported so you can check.
    """
    if not values:
        return {"count": 0}
    ordered = sorted(values)
    out: dict[str, float] = {
        "count": len(ordered),
        "mean": sum(ordered) / len(ordered),
        "min": float(ordered[0]),
        "max": float(ordered[-1]),
    }
    for q in percentiles:
        label = f"p{q:g}".replace(".", "_")
        out[label] = percentile(ordered, q)
    return out


@dataclass
class Series:
    """A sampled gauge."""

    name: str
    times: list[float] = field(default_factory=list)
    values: list[float] = field(default_factory=list)

    def add(self, t: float, v: float) -> None:
        self.times.append(t)
        self.values.append(v)

    def time_average(self, t_end: float | None = None) -> float:
        """Time-weighted mean, treating samples as step functions.

        The plain arithmetic mean of samples is wrong whenever sampling is
        irregular or the value is bursty, which is the normal case here.
        """
        if not self.times:
            return math.nan
        end = self.times[-1] if t_end is None else t_end
        total = 0.0
        span = 0.0
        for i, t in enumerate(self.times):
            nxt = self.times[i + 1] if i + 1 < len(self.times) else end
            dt = nxt - t
            if dt <= 0:
                continue
            total += self.values[i] * dt
            span += dt
        if span <= 0:
            return float(self.values[-1])
        return total / span

    def summary(self, t_end: float | None = None) -> dict[str, float]:
        out = summarize(self.values)
        out["time_avg"] = self.time_average(t_end)
        return out


class Recorder:
    """Collects events, series and counters for one simulation run."""

    def __init__(self, enabled: bool = True) -> None:
        self.enabled = enabled
        self.events: dict[str, list[dict[str, Any]]] = defaultdict(list)
        self.series: dict[str, Series] = {}
        self.counters: dict[str, float] = defaultdict(float)

    # -- writing -------------------------------------------------------------

    def event(self, kind: str, **fields: Any) -> None:
        if self.enabled:
            self.events[kind].append(fields)

    def sample(self, name: str, t: float, value: float) -> None:
        if not self.enabled:
            return
        s = self.series.get(name)
        if s is None:
            s = Series(name)
            self.series[name] = s
        s.add(t, value)

    def incr(self, name: str, amount: float = 1.0) -> None:
        self.counters[name] += amount

    # -- reading -------------------------------------------------------------

    def field(self, kind: str, name: str) -> list[float]:
        """All non-None values of one field across events of one kind."""
        return [
            row[name]
            for row in self.events.get(kind, ())
            if row.get(name) is not None
        ]

    def to_json(self, path: str, t_end: float | None = None) -> None:
        payload = {
            "counters": dict(sorted(self.counters.items())),
            "series": {
                name: self.series[name].summary(t_end)
                for name in sorted(self.series)
            },
            "event_counts": {k: len(v) for k, v in sorted(self.events.items())},
        }
        with open(path, "w", encoding="utf-8") as fh:
            json.dump(_jsonable(payload), fh, indent=2, sort_keys=True, allow_nan=False)
            fh.write("\n")

    def events_to_csv(self, kind: str, path: str) -> None:
        rows = self.events.get(kind, [])
        if not rows:
            return
        # Union of keys, in first-seen order, so column order is deterministic.
        columns: list[str] = []
        seen: set[str] = set()
        for row in rows:
            for key in row:
                if key not in seen:
                    seen.add(key)
                    columns.append(key)
        with open(path, "w", encoding="utf-8", newline="") as fh:
            writer = csv.DictWriter(fh, fieldnames=columns, extrasaction="ignore")
            writer.writeheader()
            for row in rows:
                writer.writerow(row)

    def series_to_csv(self, path: str) -> None:
        """All series in long format: name, t, value. Plots easily anywhere."""
        with open(path, "w", encoding="utf-8", newline="") as fh:
            writer = csv.writer(fh)
            writer.writerow(["series", "t", "value"])
            for name in sorted(self.series):
                s = self.series[name]
                for t, v in zip(s.times, s.values):
                    writer.writerow([name, f"{t:.6f}", f"{v:.6f}"])


def _jsonable(obj: Any) -> Any:
    """Recursively replace NaN and infinities with None.

    ``json.dump`` emits bare ``NaN`` and ``Infinity`` tokens, which no strict
    JSON parser accepts, and its ``default=`` hook is never consulted for
    floats. A run in which nothing completed produces NaN percentiles, so this
    path is normal rather than exceptional.
    """
    if isinstance(obj, float):
        return None if (math.isnan(obj) or math.isinf(obj)) else obj
    if isinstance(obj, dict):
        return {k: _jsonable(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_jsonable(v) for v in obj]
    return obj
