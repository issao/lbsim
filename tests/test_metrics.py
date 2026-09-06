import math
import os
import tempfile
import unittest
from pathlib import Path

from llmsim.sim.metrics import Recorder, Series, percentile, summarize


class TestPercentile(unittest.TestCase):
    def test_matches_numpy_linear_interpolation(self):
        values = [1, 2, 3, 4, 5]
        # Reference values from numpy.percentile(..., method="linear").
        self.assertAlmostEqual(percentile(values, 0), 1.0)
        self.assertAlmostEqual(percentile(values, 25), 2.0)
        self.assertAlmostEqual(percentile(values, 50), 3.0)
        self.assertAlmostEqual(percentile(values, 90), 4.6)
        self.assertAlmostEqual(percentile(values, 100), 5.0)

    def test_unsorted_input_is_handled(self):
        self.assertAlmostEqual(percentile([5, 1, 3, 2, 4], 50), 3.0)

    def test_empty_is_nan_not_an_error(self):
        # A run where no request completed should report nan, not crash the report.
        self.assertTrue(math.isnan(percentile([], 50)))

    def test_single_value(self):
        self.assertEqual(percentile([7.0], 99), 7.0)


class TestSummarize(unittest.TestCase):
    def test_reports_tail_percentiles(self):
        out = summarize(list(range(1, 1001)))
        self.assertEqual(out["count"], 1000)
        self.assertAlmostEqual(out["mean"], 500.5)
        self.assertAlmostEqual(out["p99"], 990.01, places=2)
        self.assertIn("p99_9", out)

    def test_empty_returns_count_only(self):
        self.assertEqual(summarize([]), {"count": 0})


class TestSeries(unittest.TestCase):
    def test_time_average_weights_by_duration(self):
        s = Series("queue_depth")
        s.add(0.0, 0.0)
        s.add(1.0, 100.0)  # held for 1s
        s.add(2.0, 0.0)
        # 0 for 1s, 100 for 1s -> 50, whereas the plain mean of samples is 33.3.
        self.assertAlmostEqual(s.time_average(t_end=2.0), 50.0)
        self.assertAlmostEqual(sum(s.values) / len(s.values), 100.0 / 3.0)

    def test_time_average_uses_t_end(self):
        s = Series("x")
        s.add(0.0, 10.0)
        self.assertAlmostEqual(s.time_average(t_end=5.0), 10.0)

    def test_time_average_of_empty_is_nan(self):
        self.assertTrue(math.isnan(Series("x").time_average()))


class TestRecorder(unittest.TestCase):
    def test_events_series_counters(self):
        rec = Recorder()
        rec.event("request", req_id=1, ttft=0.2)
        rec.event("request", req_id=2, ttft=0.4)
        rec.sample("kv_util", 1.0, 0.5)
        rec.incr("preemptions", 3)
        rec.incr("preemptions")
        self.assertEqual(rec.field("request", "ttft"), [0.2, 0.4])
        self.assertEqual(rec.counters["preemptions"], 4)
        self.assertEqual(rec.series["kv_util"].values, [0.5])

    def test_field_skips_missing_values(self):
        rec = Recorder()
        rec.event("request", req_id=1, ttft=0.2)
        rec.event("request", req_id=2)  # dropped before first token
        rec.event("request", req_id=3, ttft=None)
        self.assertEqual(rec.field("request", "ttft"), [0.2])

    def test_disabled_recorder_keeps_counters_only(self):
        rec = Recorder(enabled=False)
        rec.event("request", req_id=1)
        rec.sample("x", 0.0, 1.0)
        rec.incr("dropped")
        self.assertEqual(rec.events, {})
        self.assertEqual(rec.series, {})
        self.assertEqual(rec.counters["dropped"], 1)

    def test_csv_and_json_round_trip(self):
        rec = Recorder()
        rec.event("request", req_id=1, ttft=0.2)
        rec.event("request", req_id=2, ttft=0.4, replica="r7")  # extra column
        rec.sample("kv_util", 0.0, 0.1)
        rec.sample("kv_util", 1.0, 0.9)
        rec.incr("retries", 2)
        with tempfile.TemporaryDirectory() as d:
            ev = os.path.join(d, "requests.csv")
            se = os.path.join(d, "series.csv")
            js = os.path.join(d, "summary.json")
            rec.events_to_csv("request", ev)
            rec.series_to_csv(se)
            rec.to_json(js, t_end=1.0)
            head = Path(ev).read_text().splitlines()
            self.assertEqual(head[0], "req_id,ttft,replica")
            self.assertEqual(head[1], "1,0.2,")
            body = Path(se).read_text().splitlines()
            self.assertEqual(body[0], "series,t,value")
            self.assertEqual(len(body), 3)
            import json

            payload = json.loads(Path(js).read_text())
            self.assertEqual(payload["counters"]["retries"], 2)
            self.assertEqual(payload["event_counts"]["request"], 2)
            self.assertAlmostEqual(payload["series"]["kv_util"]["time_avg"], 0.1)

    def test_nan_serialises_as_null(self):
        rec = Recorder()
        rec.sample("x", 0.0, float("nan"))
        with tempfile.TemporaryDirectory() as d:
            js = os.path.join(d, "s.json")
            rec.to_json(js)
            self.assertIn("null", Path(js).read_text())


if __name__ == "__main__":
    unittest.main()
