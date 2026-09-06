//! The event queue.
//!
//! Ordered by `(time, priority, seq)` where `seq` is a monotonic insertion counter, so ties break by
//! insertion order and payloads are never compared. Determinism is a requirement, not an
//! optimisation: the same scenario and seed must produce identical output, and a queue that ordered
//! by anything else would break that silently.
//!
//! `std::collections::BinaryHeap`, deliberately. `bench/queue/` measured it at 54 ns per event for a
//! 6,250-entry heap, comfortably inside the 100 ns budget, and every hand-rolled d-ary heap tried
//! was slower at every size. What matters is keeping the heap small, which the two-level design in
//! `docs/ARCHITECTURE.md` section 1.4 does; the implementation should stay boring.

use crate::Nanos;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Same-timestamp ordering. Lower runs first.
pub const PRIO_HIGH: i32 = -10;
pub const PRIO_NORMAL: i32 = 0;
/// Sampling observes state *after* everything else at a timestamp has run, so a metric never
/// records a half-applied change.
pub const PRIO_OBSERVE: i32 = 100;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Key {
    at: Nanos,
    priority: i32,
    seq: u64,
}

pub struct EventQueue<E> {
    heap: BinaryHeap<Reverse<(Key, usize)>>,
    payloads: Vec<Option<E>>,
    free: Vec<usize>,
    seq: u64,
    now: Nanos,
    /// Dispatched event count. Doubles as a determinism fingerprint.
    pub dispatched: u64,
}

impl<E> EventQueue<E> {
    pub fn new(start: Nanos) -> Self {
        EventQueue {
            heap: BinaryHeap::new(),
            payloads: Vec::new(),
            free: Vec::new(),
            seq: 0,
            now: start,
            dispatched: 0,
        }
    }

    #[inline]
    pub fn now(&self) -> Nanos {
        self.now
    }

    pub fn schedule(&mut self, at: Nanos, event: E) {
        self.schedule_prio(at, PRIO_NORMAL, event)
    }

    pub fn schedule_after(&mut self, delay: Nanos, event: E) {
        let at = self.now + delay;
        self.schedule(at, event)
    }

    pub fn schedule_prio(&mut self, at: Nanos, priority: i32, event: E) {
        debug_assert!(at >= self.now, "scheduling into the past");
        let at = at.max(self.now);
        self.seq += 1;
        let slot = match self.free.pop() {
            Some(i) => {
                self.payloads[i] = Some(event);
                i
            }
            None => {
                self.payloads.push(Some(event));
                self.payloads.len() - 1
            }
        };
        self.heap.push(Reverse((
            Key {
                at,
                priority,
                seq: self.seq,
            },
            slot,
        )));
    }

    pub fn peek_time(&self) -> Option<Nanos> {
        self.heap.peek().map(|Reverse((k, _))| k.at)
    }

    pub fn pop(&mut self) -> Option<(Nanos, E)> {
        let Reverse((key, slot)) = self.heap.pop()?;
        self.now = key.at;
        self.dispatched += 1;
        let event = self.payloads[slot].take().expect("slot already consumed");
        self.free.push(slot);
        Some((key.at, event))
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}
