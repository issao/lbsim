"""Deterministic discrete-event simulation engine.

Design notes
------------
*Determinism* is the top requirement. Two runs with the same inputs must produce
byte-identical output, so:

- The ready queue is a heap keyed by ``(time, priority, seq)``. ``seq`` is a
  monotonically increasing insertion counter, so ties break by insertion order
  and payloads are never compared. No dict or set iteration order ever
  influences scheduling.
- Nothing reads a wall clock. ``Simulator.now`` is the only notion of time.

*Processes* are generators. They yield to wait:

    def replica_loop(sim):
        while True:
            yield sim.timeout(0.02)      # wait a fixed delay
            step()

    def client(sim, server):
        call = sim.process(server.handle(req))
        outcome = yield sim.any_of([call, sim.timeout(30.0)])
        if outcome.index == 1:
            call.interrupt("client timeout")

A generator may yield:

- an :class:`Event`  -> resume when it fires, with its value
- a ``float``/``int`` -> shorthand for ``sim.timeout(delay)``
- ``None``            -> resume at the same simulated time, after other
                         same-time work already queued (a yield point)

A :class:`Process` is itself an :class:`Event`, so ``yield other_process`` waits
for that process to finish and evaluates to its return value.
"""

from __future__ import annotations

import heapq
from typing import Any, Callable, Generator, Iterable, Sequence

Time = float

# Priorities for same-timestamp ordering. Lower runs first.
PRIO_HIGH = -10
PRIO_NORMAL = 0
PRIO_LOW = 10
#: Sampling and bookkeeping should observe state *after* everything else at a
#: given timestamp has run.
PRIO_OBSERVE = 100


class Interrupt(Exception):
    """Thrown into a process by :meth:`Process.interrupt`.

    ``args[0]`` is the cause passed by the interrupter.
    """

    @property
    def cause(self) -> Any:
        return self.args[0] if self.args else None


class SimulationError(RuntimeError):
    """Misuse of the engine, e.g. scheduling into the past."""


class Event:
    """A one-shot condition that processes can wait on.

    An event is *pending* until :meth:`succeed` or :meth:`fail` is called, after
    which it is *fired* and carries a value (or an exception). Waiting on an
    already-fired event resumes the waiter at the current time.
    """

    __slots__ = ("_callbacks", "_exception", "_sim", "_state", "_value", "name")

    _PENDING = 0
    _FIRED = 1
    _CANCELLED = 2

    def __init__(self, sim: "Simulator", name: str = "") -> None:
        self._sim = sim
        self.name = name
        self._state = Event._PENDING
        self._value: Any = None
        self._exception: BaseException | None = None
        self._callbacks: list[Callable[[Event], None]] = []

    # -- state ---------------------------------------------------------------

    @property
    def fired(self) -> bool:
        return self._state == Event._FIRED

    @property
    def cancelled(self) -> bool:
        return self._state == Event._CANCELLED

    @property
    def pending(self) -> bool:
        return self._state == Event._PENDING

    @property
    def value(self) -> Any:
        if self._state != Event._FIRED:
            raise SimulationError(f"event {self!r} has no value yet")
        return self._value

    # -- firing --------------------------------------------------------------

    def succeed(self, value: Any = None) -> "Event":
        if self._state == Event._CANCELLED:
            return self
        if self._state == Event._FIRED:
            raise SimulationError(f"event {self!r} already fired")
        self._state = Event._FIRED
        self._value = value
        self._sim._fire(self)
        return self

    def fail(self, exception: BaseException) -> "Event":
        if self._state == Event._CANCELLED:
            return self
        if self._state == Event._FIRED:
            raise SimulationError(f"event {self!r} already fired")
        self._state = Event._FIRED
        self._exception = exception
        self._sim._fire(self)
        return self

    def cancel(self) -> None:
        """Make this event never fire. Waiters are left waiting forever.

        Used for timers that are no longer needed. Cancelling a fired event is
        a no-op, which keeps race-free cleanup simple for callers.
        """
        if self._state == Event._PENDING:
            self._state = Event._CANCELLED
            self._callbacks.clear()

    # -- waiting -------------------------------------------------------------

    def add_callback(self, fn: Callable[["Event"], None]) -> None:
        if self._state == Event._FIRED:
            self._sim._schedule_raw(0.0, PRIO_NORMAL, lambda: fn(self))
        elif self._state == Event._PENDING:
            self._callbacks.append(fn)
        # cancelled: never call back

    def __repr__(self) -> str:
        state = {0: "pending", 1: "fired", 2: "cancelled"}[self._state]
        label = f" {self.name}" if self.name else ""
        return f"<Event{label} {state}>"


class Process(Event):
    """A generator driven by the simulator. Also an Event: fires on return."""

    __slots__ = ("_gen", "_interrupt", "_target", "_waiting_on")

    def __init__(self, sim: "Simulator", gen: Generator, name: str = "") -> None:
        super().__init__(sim, name or getattr(gen, "__name__", "process"))
        self._gen = gen
        self._waiting_on: Event | None = None
        # Start on the next scheduler pass, not inline, so the caller can hold
        # the handle before the body runs.
        sim._schedule_raw(0.0, PRIO_NORMAL, lambda: self._resume(None, None))

    def interrupt(self, cause: Any = None) -> None:
        """Throw :class:`Interrupt` into the process at the current time.

        No-op if the process has already finished. The event it was waiting on
        is detached, so a later firing of that event does not resume it twice.
        """
        if self._state != Event._PENDING:
            return
        waiting = self._waiting_on
        self._waiting_on = None
        if waiting is not None:
            # Detach: if `waiting` fires later we must not resume this process.
            waiting._callbacks = [
                cb for cb in waiting._callbacks if getattr(cb, "_owner", None) is not self
            ]
        self._sim._schedule_raw(
            0.0, PRIO_HIGH, lambda: self._resume(None, Interrupt(cause))
        )

    # -- driving -------------------------------------------------------------

    def _resume(self, value: Any, throw: BaseException | None) -> None:
        if self._state != Event._PENDING:
            return
        self._waiting_on = None
        try:
            if throw is not None:
                yielded = self._gen.throw(throw)
            else:
                yielded = self._gen.send(value)
        except StopIteration as stop:
            super().succeed(stop.value)
            return
        except BaseException as exc:  # noqa: BLE001 - propagated to waiters
            super().fail(exc)
            return
        self._wait_for(yielded)

    def _wait_for(self, yielded: Any) -> None:
        sim = self._sim
        if yielded is None:
            sim._schedule_raw(0.0, PRIO_LOW, lambda: self._resume(None, None))
            return
        if isinstance(yielded, (int, float)):
            yielded = sim.timeout(float(yielded))
        elif isinstance(yielded, (list, tuple)):
            yielded = sim.all_of(yielded)
        if not isinstance(yielded, Event):
            self._resume(
                None,
                SimulationError(
                    f"process {self.name!r} yielded {yielded!r}; expected Event, "
                    "number, None, or a sequence of Events"
                ),
            )
            return
        self._waiting_on = yielded

        def on_fire(ev: Event, _self: "Process" = self) -> None:
            if _self._waiting_on is not ev:
                return  # interrupted or detached in the meantime
            _self._resume(ev._value, ev._exception)

        on_fire._owner = self  # type: ignore[attr-defined]
        yielded.add_callback(on_fire)


class _AnyResult:
    """Value of an :meth:`Simulator.any_of` event."""

    __slots__ = ("event", "index", "value")

    def __init__(self, index: int, event: Event, value: Any) -> None:
        self.index = index
        self.event = event
        self.value = value

    def __repr__(self) -> str:
        return f"<AnyResult index={self.index} value={self.value!r}>"


class Simulator:
    """The event loop and the only source of simulated time."""

    def __init__(self, start: Time = 0.0) -> None:
        self.now: Time = float(start)
        self._heap: list[tuple[Time, int, int, Callable[[], None]]] = []
        self._seq = 0
        self._stop = False
        #: Number of events dispatched. Useful as a determinism fingerprint.
        self.steps = 0

    # -- scheduling ----------------------------------------------------------

    def _schedule_raw(
        self, delay: Time, priority: int, fn: Callable[[], None]
    ) -> None:
        if delay < 0:
            raise SimulationError(f"negative delay {delay}")
        self._seq += 1
        heapq.heappush(self._heap, (self.now + delay, priority, self._seq, fn))

    def schedule(
        self, delay: Time, fn: Callable[[], None], priority: int = PRIO_NORMAL
    ) -> None:
        """Run ``fn()`` after ``delay``. The plain callback path."""
        self._schedule_raw(delay, priority, fn)

    def event(self, name: str = "") -> Event:
        """A pending event that someone will fire explicitly."""
        return Event(self, name)

    def timeout(self, delay: Time, value: Any = None, name: str = "") -> Event:
        """An event that fires by itself after ``delay``."""
        ev = Event(self, name or f"timeout({delay})")
        self._schedule_raw(delay, PRIO_NORMAL, lambda: ev.succeed(value))
        return ev

    def process(self, gen: Generator, name: str = "") -> Process:
        """Start ``gen`` as a process. Returns a handle that is also an Event."""
        return Process(self, gen, name)

    def any_of(self, events: Sequence[Event], name: str = "") -> Event:
        """Fire when the first of ``events`` fires.

        Value is an ``_AnyResult`` with ``.index``, ``.event`` and ``.value``.
        On a tie the lowest index wins, which keeps the outcome deterministic.
        Losing events are *not* cancelled; the caller decides what to do with
        them, because a losing timer and a losing RPC need different cleanup.
        """
        events = list(events)
        if not events:
            raise SimulationError("any_of() needs at least one event")
        out = Event(self, name or "any_of")

        def make_cb(index: int, ev: Event) -> Callable[[Event], None]:
            def cb(_: Event) -> None:
                if out.pending:
                    out.succeed(_AnyResult(index, ev, ev._value))

            return cb

        for i, ev in enumerate(events):
            ev.add_callback(make_cb(i, ev))
        return out

    def all_of(self, events: Sequence[Event], name: str = "") -> Event:
        """Fire when every one of ``events`` has fired. Value is a list of values."""
        events = list(events)
        out = Event(self, name or "all_of")
        if not events:
            out.succeed([])
            return out
        remaining = [len(events)]
        values: list[Any] = [None] * len(events)

        def make_cb(index: int) -> Callable[[Event], None]:
            def cb(ev: Event) -> None:
                values[index] = ev._value
                remaining[0] -= 1
                if remaining[0] == 0 and out.pending:
                    out.succeed(values)

            return cb

        for i, ev in enumerate(events):
            ev.add_callback(make_cb(i))
        return out

    # -- internals -----------------------------------------------------------

    def _fire(self, ev: Event) -> None:
        callbacks, ev._callbacks = ev._callbacks, []
        for cb in callbacks:
            self._schedule_raw(0.0, PRIO_NORMAL, lambda cb=cb, ev=ev: cb(ev))

    # -- running -------------------------------------------------------------

    def peek(self) -> Time | None:
        """Time of the next scheduled item, or None if the queue is empty."""
        return self._heap[0][0] if self._heap else None

    def stop(self) -> None:
        """Ask :meth:`run` to return after the current callback."""
        self._stop = True

    def run(self, until: Time | None = None, max_steps: int | None = None) -> Time:
        """Advance until the queue empties, ``until`` is reached, or stopped.

        ``until`` is inclusive of events scheduled exactly at that time, which
        makes a run of ``until=T`` reproducible regardless of float rounding at
        the boundary. Returns the final simulated time.
        """
        self._stop = False
        while self._heap and not self._stop:
            if max_steps is not None and self.steps >= max_steps:
                break
            when = self._heap[0][0]
            if until is not None and when > until:
                break
            _, _, _, fn = heapq.heappop(self._heap)
            if when < self.now:  # pragma: no cover - defensive
                raise SimulationError(f"time went backwards: {when} < {self.now}")
            self.now = when
            self.steps += 1
            fn()
        if until is not None and not self._stop:
            self.now = max(self.now, float(until))
        return self.now
