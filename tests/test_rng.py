"""Stream independence is the property that makes A/B comparison valid."""

import statistics
import unittest

from llmsim.sim.rng import (
    RngRegistry,
    bounded_int,
    exponential,
    lognormal,
    sample_bimodal,
    zipf,
)


class TestRngRegistry(unittest.TestCase):
    def test_same_seed_same_sequence(self):
        a = RngRegistry(1234).stream("arrival")
        b = RngRegistry(1234).stream("arrival")
        self.assertEqual([a.random() for _ in range(20)], [b.random() for _ in range(20)])

    def test_different_seed_different_sequence(self):
        a = RngRegistry(1).stream("arrival")
        b = RngRegistry(2).stream("arrival")
        self.assertNotEqual([a.random() for _ in range(20)], [b.random() for _ in range(20)])

    def test_named_streams_are_independent(self):
        reg = RngRegistry(7)
        # Draining one stream must not shift another. This is the whole point.
        reference = RngRegistry(7).stream("prompt")
        expected = [reference.random() for _ in range(5)]
        for _ in range(1000):
            reg.stream("arrival").random()
        actual = [reg.stream("prompt").random() for _ in range(5)]
        self.assertEqual(expected, actual)

    def test_stream_is_cached(self):
        reg = RngRegistry(9)
        self.assertIs(reg.stream("x"), reg.stream("x"))

    def test_seeds_are_stable_across_processes(self):
        # blake2b, not Python's hash(), which is salted per process for strings.
        self.assertEqual(RngRegistry(42).derive_seed("arrival"), RngRegistry(42).derive_seed("arrival"))
        self.assertNotEqual(RngRegistry(42).derive_seed("arrival"), RngRegistry(42).derive_seed("prompt"))

    def test_child_registries_are_independent(self):
        reg = RngRegistry(3)
        r0 = reg.child("replica-0").stream("fail")
        r1 = reg.child("replica-1").stream("fail")
        self.assertNotEqual([r0.random() for _ in range(10)], [r1.random() for _ in range(10)])


class TestDistributions(unittest.TestCase):
    def test_exponential_mean(self):
        rng = RngRegistry(11).stream("s")
        draws = [exponential(rng, 0.05) for _ in range(200_000)]
        self.assertAlmostEqual(statistics.fmean(draws), 0.05, delta=0.002)

    def test_lognormal_matches_requested_mean_and_cv(self):
        rng = RngRegistry(13).stream("s")
        draws = [lognormal(rng, 800.0, 1.5) for _ in range(200_000)]
        mean = statistics.fmean(draws)
        cv = statistics.stdev(draws) / mean
        self.assertAlmostEqual(mean, 800.0, delta=25.0)
        self.assertAlmostEqual(cv, 1.5, delta=0.15)

    def test_lognormal_is_right_skewed(self):
        rng = RngRegistry(17).stream("s")
        draws = sorted(lognormal(rng, 500.0, 2.0) for _ in range(50_000))
        median = draws[len(draws) // 2]
        self.assertLess(median, statistics.fmean(draws))  # skew, as intended

    def test_lognormal_degenerate_cases(self):
        rng = RngRegistry(19).stream("s")
        self.assertEqual(lognormal(rng, 0.0, 1.0), 0.0)
        self.assertEqual(lognormal(rng, 100.0, 0.0), 100.0)

    def test_bounded_int_clamps(self):
        self.assertEqual(bounded_int(1e9, 1, 128_000), 128_000)
        self.assertEqual(bounded_int(-5.0, 1, 128_000), 1)
        self.assertEqual(bounded_int(12.6, 1, 100), 13)

    def test_zipf_is_skewed_towards_index_zero(self):
        rng = RngRegistry(23).stream("s")
        draws = [zipf(rng, 20, 1.2) for _ in range(20_000)]
        self.assertGreater(draws.count(0), draws.count(19) * 5)
        self.assertTrue(all(0 <= d < 20 for d in draws))

    def test_zipf_handles_degenerate_n(self):
        rng = RngRegistry(29).stream("s")
        self.assertEqual(zipf(rng, 1), 0)

    def test_bimodal_produces_two_modes(self):
        rng = RngRegistry(31).stream("s")
        draws = [sample_bimodal(rng, 0.1, 500.0, 40_000.0, 0.4) for _ in range(20_000)]
        long_draws = [d for d in draws if d > 10_000]
        self.assertGreater(len(long_draws), 1_400)
        self.assertLess(len(long_draws), 2_600)


if __name__ == "__main__":
    unittest.main()
