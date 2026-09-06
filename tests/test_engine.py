"""Engine behaviour, with determinism as the headline property."""

import unittest

from llmsim.sim.engine import (
    PRIO_HIGH,
    PRIO_OBSERVE,
    Event,
    Interrupt,
    SimulationError,
    Simulator,
)


class TestScheduling(unittest.TestCase):
    def test_callbacks_run_in_time_order(self):
        sim = Simulator()
        log = []
        sim.schedule(3.0, lambda: log.append(("c", sim.now)))
        sim.schedule(1.0, lambda: log.append(("a", sim.now)))
        sim.schedule(2.0, lambda: log.append(("b", sim.now)))
        sim.run()
        self.assertEqual(log, [("a", 1.0), ("b", 2.0), ("c", 3.0)])

    def test_same_time_ties_break_by_insertion_order(self):
        sim = Simulator()
        log = []
        for i in range(5):
            sim.schedule(1.0, lambda i=i: log.append(i))
        sim.run()
        self.assertEqual(log, [0, 1, 2, 3, 4])

    def test_priority_orders_within_a_timestamp(self):
        sim = Simulator()
        log = []
        sim.schedule(1.0, lambda: log.append("observe"), priority=PRIO_OBSERVE)
        sim.schedule(1.0, lambda: log.append("normal"))
        sim.schedule(1.0, lambda: log.append("high"), priority=PRIO_HIGH)
        sim.run()
        self.assertEqual(log, ["high", "normal", "observe"])

    def test_negative_delay_rejected(self):
        sim = Simulator()
        with self.assertRaises(SimulationError):
            sim.schedule(-1.0, lambda: None)

    def test_run_until_stops_early_and_advances_clock(self):
        sim = Simulator()
        log = []
        sim.schedule(1.0, lambda: log.append(1))
        sim.schedule(10.0, lambda: log.append(10))
        end = sim.run(until=5.0)
        self.assertEqual(log, [1])
        self.assertEqual(end, 5.0)
        # The un-run event is still queued and runs on a later run().
        sim.run()
        self.assertEqual(log, [1, 10])

    def test_run_until_includes_boundary(self):
        sim = Simulator()
        log = []
        sim.schedule(5.0, lambda: log.append("at-boundary"))
        sim.run(until=5.0)
        self.assertEqual(log, ["at-boundary"])

    def test_stop_halts_run(self):
        sim = Simulator()
        log = []
        sim.schedule(1.0, lambda: (log.append(1), sim.stop()))
        sim.schedule(2.0, lambda: log.append(2))
        sim.run()
        self.assertEqual(log, [1])


class TestProcesses(unittest.TestCase):
    def test_process_waits_and_returns_value(self):
        sim = Simulator()

        def body(sim):
            yield sim.timeout(2.0)
            return 42

        p = sim.process(body(sim))
        sim.run()
        self.assertTrue(p.fired)
        self.assertEqual(p.value, 42)
        self.assertEqual(sim.now, 2.0)

    def test_yield_number_is_a_timeout(self):
        sim = Simulator()

        def body(sim):
            yield 1.5
            yield 1.5

        sim.process(body(sim))
        sim.run()
        self.assertEqual(sim.now, 3.0)

    def test_yield_none_is_a_same_time_yield_point(self):
        sim = Simulator()
        log = []

        def body(sim):
            log.append(("before", sim.now))
            yield None
            log.append(("after", sim.now))

        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, [("before", 0.0), ("after", 0.0)])

    def test_process_can_wait_on_another_process(self):
        sim = Simulator()

        def inner(sim):
            yield sim.timeout(3.0)
            return "inner-result"

        def outer(sim):
            result = yield sim.process(inner(sim))
            return result

        p = sim.process(outer(sim))
        sim.run()
        self.assertEqual(p.value, "inner-result")
        self.assertEqual(sim.now, 3.0)

    def test_exception_in_process_propagates_to_waiter(self):
        sim = Simulator()

        def inner(sim):
            yield sim.timeout(1.0)
            raise ValueError("boom")

        seen = []

        def outer(sim):
            try:
                yield sim.process(inner(sim))
            except ValueError as exc:
                seen.append(str(exc))

        sim.process(outer(sim))
        sim.run()
        self.assertEqual(seen, ["boom"])

    def test_explicit_event_wakes_waiter(self):
        sim = Simulator()
        gate = Event(sim, "gate")
        log = []

        def waiter(sim):
            value = yield gate
            log.append((value, sim.now))

        sim.process(waiter(sim))
        sim.schedule(4.0, lambda: gate.succeed("open"))
        sim.run()
        self.assertEqual(log, [("open", 4.0)])

    def test_waiting_on_already_fired_event_resumes_at_same_time(self):
        sim = Simulator()
        done = Event(sim).succeed("v")
        log = []

        def waiter(sim):
            value = yield done
            log.append((value, sim.now))

        sim.process(waiter(sim))
        sim.run()
        self.assertEqual(log, [("v", 0.0)])


class TestInterrupt(unittest.TestCase):
    def test_interrupt_raises_inside_process(self):
        sim = Simulator()
        log = []

        def victim(sim):
            try:
                yield sim.timeout(100.0)
                log.append("finished")
            except Interrupt as exc:
                log.append(("interrupted", exc.cause, sim.now))

        p = sim.process(victim(sim))
        sim.schedule(5.0, lambda: p.interrupt("cancelled"))
        sim.run()
        self.assertEqual(log, [("interrupted", "cancelled", 5.0)])

    def test_interrupted_process_is_not_resumed_by_the_stale_timer(self):
        # The bug this guards against: interrupt at t=5, the timeout it was
        # waiting on still fires at t=100, and the process runs twice.
        sim = Simulator()
        log = []

        def victim(sim):
            try:
                yield sim.timeout(100.0)
            except Interrupt:
                log.append("interrupted")
            log.append(("tail", sim.now))

        p = sim.process(victim(sim))
        sim.schedule(5.0, lambda: p.interrupt())
        sim.run()
        self.assertEqual(log, ["interrupted", ("tail", 5.0)])

    def test_interrupt_after_completion_is_a_noop(self):
        sim = Simulator()

        def body(sim):
            yield sim.timeout(1.0)

        p = sim.process(body(sim))
        sim.run()
        p.interrupt("late")  # must not raise
        sim.run()


class TestCombinators(unittest.TestCase):
    def test_any_of_returns_first_and_its_index(self):
        sim = Simulator()
        log = []

        def body(sim):
            result = yield sim.any_of([sim.timeout(9.0, "slow"), sim.timeout(2.0, "fast")])
            log.append((result.index, result.value, sim.now))

        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, [(1, "fast", 2.0)])

    def test_any_of_tie_breaks_to_lowest_index(self):
        sim = Simulator()
        log = []

        def body(sim):
            result = yield sim.any_of([sim.timeout(2.0, "a"), sim.timeout(2.0, "b")])
            log.append(result.index)

        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, [0])

    def test_all_of_waits_for_the_slowest(self):
        sim = Simulator()
        log = []

        def body(sim):
            values = yield sim.all_of(
                [sim.timeout(1.0, "a"), sim.timeout(5.0, "b"), sim.timeout(3.0, "c")]
            )
            log.append((values, sim.now))

        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, [(["a", "b", "c"], 5.0)])

    def test_yielding_a_list_means_all_of(self):
        sim = Simulator()
        log = []

        def body(sim):
            values = yield [sim.timeout(1.0, "a"), sim.timeout(4.0, "b")]
            log.append((values, sim.now))

        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, [(["a", "b"], 4.0)])

    def test_cancelled_event_never_fires(self):
        sim = Simulator()
        log = []
        ev = sim.timeout(1.0, "x")

        def body(sim):
            yield sim.timeout(0.5)
            ev.cancel()
            yield sim.timeout(5.0)
            log.append("done")

        def waiter(sim):
            yield ev
            log.append("should-not-happen")

        sim.process(waiter(sim))
        sim.process(body(sim))
        sim.run()
        self.assertEqual(log, ["done"])


class TestDeterminism(unittest.TestCase):
    def _run(self):
        """A run with concurrent processes, explicit events and interrupts."""
        sim = Simulator()
        trace = []

        def worker(sim, name, period, count):
            for i in range(count):
                yield sim.timeout(period)
                trace.append((round(sim.now, 6), name, i))

        def canceller(sim, victim):
            yield sim.timeout(0.35)
            victim.interrupt("stop")
            trace.append((round(sim.now, 6), "cancel", -1))

        a = sim.process(worker(sim, "a", 0.1, 10))
        sim.process(worker(sim, "b", 0.15, 10))
        sim.process(worker(sim, "c", 0.1, 10))  # ties with "a" on purpose
        sim.process(canceller(sim, a))
        sim.run()
        return trace, sim.steps

    def test_repeated_runs_are_identical(self):
        first = self._run()
        for _ in range(5):
            self.assertEqual(self._run(), first)

    def test_step_count_is_a_stable_fingerprint(self):
        _, steps_a = self._run()
        _, steps_b = self._run()
        self.assertEqual(steps_a, steps_b)


if __name__ == "__main__":
    unittest.main()
