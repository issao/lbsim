//! `queue::EventQueue`, tested directly.
//!
//! Every other property in this suite rests on this one. The event queue decides the order in which
//! the simulated world happens, and its ordering key is `(time, priority, insertion)` precisely so
//! that no tie is ever broken by anything ambient. Payloads are never compared, so a scenario cannot
//! become order-dependent on a struct's field values or on a heap's internal layout.

use lbsim::queue::{EventQueue, PRIO_HIGH, PRIO_NORMAL, PRIO_OBSERVE};
use lbsim::Nanos;

const T0: Nanos = 1_000;

#[test]
fn events_come_out_in_time_order_regardless_of_insertion_order() {
    let mut q: EventQueue<&str> = EventQueue::new(T0);
    q.schedule(T0 + 500, "e");
    q.schedule(T0 + 10, "b");
    q.schedule(T0 + 900, "f");
    q.schedule(T0, "a");
    q.schedule(T0 + 100, "d");
    q.schedule(T0 + 20, "c");

    let mut seen = Vec::new();
    let mut last = 0;
    while let Some((at, e)) = q.pop() {
        assert!(at >= last, "time went backwards: {at} after {last}");
        last = at;
        seen.push(e);
    }
    assert_eq!(seen, vec!["a", "b", "c", "d", "e", "f"]);
    assert!(q.is_empty());
}

/// Ties at the same timestamp break by insertion order.
///
/// This is the load-bearing determinism guarantee. A great many events in a run land on exactly the
/// same nanosecond — a step completing while a telemetry tick fires, several admissions at one instant
/// — and if their order came from anywhere else (heap layout, payload comparison) two runs of the same
/// scenario would diverge for reasons no scenario file records.
#[test]
fn same_timestamp_ties_break_by_insertion_order() {
    let mut q: EventQueue<u32> = EventQueue::new(T0);
    for i in 0..64 {
        q.schedule(T0 + 42, i);
    }
    let mut seen = Vec::new();
    while let Some((at, e)) = q.pop() {
        assert_eq!(at, T0 + 42);
        seen.push(e);
    }
    assert_eq!(seen, (0..64).collect::<Vec<u32>>(), "same-timestamp events were reordered");
}

/// Priority orders events inside one timestamp, and insertion order still breaks ties inside one
/// priority. `PRIO_OBSERVE` last is what makes a sample read state after everything at that instant
/// has been applied, so a gauge never records a half-applied change.
#[test]
fn priority_orders_events_within_a_timestamp() {
    let mut q: EventQueue<&str> = EventQueue::new(T0);
    // Deliberately inserted worst-first, so insertion order alone would give the wrong answer.
    q.schedule_prio(T0 + 5, PRIO_OBSERVE, "observe_1");
    q.schedule_prio(T0 + 5, PRIO_NORMAL, "normal_1");
    q.schedule_prio(T0 + 5, PRIO_HIGH, "high_1");
    q.schedule_prio(T0 + 5, PRIO_OBSERVE, "observe_2");
    q.schedule_prio(T0 + 5, PRIO_HIGH, "high_2");
    q.schedule_prio(T0 + 5, PRIO_NORMAL, "normal_2");

    let mut seen = Vec::new();
    while let Some((_, e)) = q.pop() {
        seen.push(e);
    }
    assert_eq!(
        seen,
        vec!["high_1", "high_2", "normal_1", "normal_2", "observe_1", "observe_2"]
    );
}

/// Priority must never reorder across timestamps. A high-priority event later in time is still later:
/// otherwise the simulation would execute the future before the present.
#[test]
fn priority_never_overrides_time() {
    let mut q: EventQueue<&str> = EventQueue::new(T0);
    q.schedule_prio(T0 + 1_000, PRIO_HIGH, "urgent_but_later");
    q.schedule_prio(T0 + 1, PRIO_OBSERVE, "lazy_but_sooner");

    assert_eq!(q.pop().map(|(_, e)| e), Some("lazy_but_sooner"));
    assert_eq!(q.pop().map(|(_, e)| e), Some("urgent_but_later"));
}

/// `now` tracks the last dispatched event, `peek_time` the next one, and `schedule_after` is relative
/// to the current instant. Getting `now` wrong would silently shift every relative delay in the model.
#[test]
fn now_and_peek_track_the_dispatch_position() {
    let mut q: EventQueue<&str> = EventQueue::new(T0);
    assert_eq!(q.now(), T0);
    assert_eq!(q.peek_time(), None);

    q.schedule(T0 + 100, "a");
    q.schedule(T0 + 300, "c");
    assert_eq!(q.peek_time(), Some(T0 + 100));

    let (at, e) = q.pop().unwrap();
    assert_eq!((at, e), (T0 + 100, "a"));
    assert_eq!(q.now(), T0 + 100);

    q.schedule_after(50, "b");
    assert_eq!(q.peek_time(), Some(T0 + 150), "schedule_after did not use the current time");
    assert_eq!(q.pop().map(|(t, e)| (t, e)), Some((T0 + 150, "b")));
    assert_eq!(q.pop().map(|(t, e)| (t, e)), Some((T0 + 300, "c")));
}

/// `dispatched` is used as a determinism fingerprint by `RunResult::events`, so it must count exactly
/// the events dispatched — not the events scheduled, and not the events remaining.
#[test]
fn dispatched_counts_exactly_the_events_popped() {
    let mut q: EventQueue<u32> = EventQueue::new(T0);
    assert_eq!(q.dispatched, 0);
    for i in 0..10 {
        q.schedule(T0 + i as Nanos, i);
    }
    assert_eq!(q.dispatched, 0, "scheduling must not count as dispatching");
    for expected in 1..=10_u64 {
        q.pop().unwrap();
        assert_eq!(q.dispatched, expected);
    }
    assert!(q.pop().is_none());
    assert_eq!(q.dispatched, 10, "an empty pop must not be counted");
}

/// Payload slots are recycled as events are consumed. Interleaving heavily is the case where a slot
/// bookkeeping bug would hand back the wrong payload, and the simulator interleaves constantly:
/// nearly every event it dispatches schedules another.
#[test]
fn recycled_payload_slots_never_return_the_wrong_event() {
    let mut q: EventQueue<u64> = EventQueue::new(0);
    let mut next_value = 0_u64;
    let mut expected: Vec<u64> = Vec::new();
    let mut got: Vec<u64> = Vec::new();

    // Everything is scheduled at a strictly increasing time so the expected order is unambiguous.
    let mut t: Nanos = 0;
    for round in 0..200 {
        for _ in 0..(round % 5 + 1) {
            t += 1;
            q.schedule(t, next_value);
            expected.push(next_value);
            next_value += 1;
        }
        if round % 2 == 0 {
            if let Some((_, v)) = q.pop() {
                got.push(v);
            }
        }
    }
    while let Some((_, v)) = q.pop() {
        got.push(v);
    }
    assert_eq!(got, expected, "slot recycling returned payloads out of order or duplicated");
    assert_eq!(q.len(), 0);
}

/// Scheduling at the current instant is legal and is what a zero-delay reaction does — a replica
/// stepping immediately on admission, for instance. It must land after everything already dispatched
/// at that instant, not before.
#[test]
fn scheduling_at_the_current_instant_lands_after_the_current_event() {
    let mut q: EventQueue<&str> = EventQueue::new(0);
    q.schedule(500, "trigger");
    let (at, e) = q.pop().unwrap();
    assert_eq!((at, e), (500, "trigger"));

    q.schedule(at, "reaction");
    q.schedule(at, "second_reaction");
    assert_eq!(q.pop().map(|(t, e)| (t, e)), Some((500, "reaction")));
    assert_eq!(q.pop().map(|(t, e)| (t, e)), Some((500, "second_reaction")));
    assert!(q.is_empty());
}
