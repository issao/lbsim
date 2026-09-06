//! Measure the event-queue throughput that docs/ARCHITECTURE.md section 1.4 depends on.
//!
//! The plan assumes ~100 ns per event. If a flat binary heap over ~1.7 M future events costs
//! 1-2 us per operation, as suspected, the 20x-realtime target needs the two-level structure
//! instead: each replica holds its own next-event time, and a small heap orders only the
//! replicas, so the hot structure stays cache-resident no matter how many requests are in
//! flight.
//!
//! Workload shape mirrors the real one: pop the earliest event, do trivial work, push that
//! actor's next event a short while later. That is exactly the epoch-advance pattern.
//!
//! Ordering is by `(time_ns, seq)` in every variant, so ties break by insertion order and
//! payloads are never compared. Determinism is a requirement, not an optimisation.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

/// Deterministic, cheap, and not part of what is being measured.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// Interval in nanoseconds, spread over roughly 1 ms to 1 s, like real epoch durations.
    fn interval(&mut self) -> u64 {
        1_000_000 + (self.next_u64() % 999_000_000)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    time_ns: u64,
    seq: u64,
    actor: u32,
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.time_ns
            .cmp(&other.time_ns)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Variant A: one flat binary heap holding every future event.
///
/// `per_actor` future events per actor models a simulator that eagerly schedules per-request
/// timers: client timeouts, deadlines, retries. That is how the queue reaches millions of
/// entries.
fn flat_heap(actors: u32, per_actor: u32, iters: u64, seed: u64) -> (f64, u64) {
    let mut rng = Rng(seed);
    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::with_capacity((actors * per_actor) as usize);
    let mut seq = 0u64;
    for a in 0..actors {
        for _ in 0..per_actor {
            seq += 1;
            heap.push(Reverse(Key { time_ns: rng.interval(), seq, actor: a }));
        }
    }
    let start = Instant::now();
    let mut sink = 0u64;
    for _ in 0..iters {
        let Reverse(k) = heap.pop().unwrap();
        let now = k.time_ns;
        sink = sink.wrapping_add(k.actor as u64);
        seq += 1;
        heap.push(Reverse(Key { time_ns: now + rng.interval(), seq, actor: k.actor }));
    }
    (start.elapsed().as_secs_f64() / iters as f64 * 1e9, sink)
}

/// Variant B: two-level. A 4-ary implicit heap over actors only; each actor's own future
/// events live in its own small structure and never enter the global heap.
///
/// 4-ary rather than binary because the shallower tree means fewer cache lines touched per
/// sift, and siblings share a line.
struct FourAry {
    /// (time_ns, seq, actor)
    nodes: Vec<(u64, u64, u32)>,
    /// Position of each actor in `nodes`, so an actor's key can be updated in place.
    pos: Vec<u32>,
}

impl FourAry {
    fn with_actors(n: u32) -> Self {
        FourAry { nodes: Vec::with_capacity(n as usize), pos: vec![u32::MAX; n as usize] }
    }

    fn less(a: (u64, u64, u32), b: (u64, u64, u32)) -> bool {
        (a.0, a.1) < (b.0, b.1)
    }

    fn push(&mut self, time_ns: u64, seq: u64, actor: u32) {
        let i = self.nodes.len();
        self.nodes.push((time_ns, seq, actor));
        self.pos[actor as usize] = i as u32;
        self.sift_up(i);
    }

    fn sift_up(&mut self, mut i: usize) {
        while i > 0 {
            let parent = (i - 1) / 4;
            if Self::less(self.nodes[i], self.nodes[parent]) {
                self.swap(i, parent);
                i = parent;
            } else {
                break;
            }
        }
    }

    fn sift_down(&mut self, mut i: usize) {
        let n = self.nodes.len();
        loop {
            let first = 4 * i + 1;
            if first >= n {
                break;
            }
            let mut best = first;
            for c in (first + 1)..(first + 4).min(n) {
                if Self::less(self.nodes[c], self.nodes[best]) {
                    best = c;
                }
            }
            if Self::less(self.nodes[best], self.nodes[i]) {
                self.swap(i, best);
                i = best;
            } else {
                break;
            }
        }
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.nodes.swap(a, b);
        self.pos[self.nodes[a].2 as usize] = a as u32;
        self.pos[self.nodes[b].2 as usize] = b as u32;
    }

    fn peek(&self) -> (u64, u64, u32) {
        self.nodes[0]
    }

    /// The operation the simulator actually performs: the earliest actor is advanced and
    /// re-keyed. No pop-then-push pair, so the tree is touched once instead of twice.
    fn replace_min(&mut self, time_ns: u64, seq: u64) {
        self.nodes[0].0 = time_ns;
        self.nodes[0].1 = seq;
        self.sift_down(0);
    }
}

/// Variant D: a d-ary implicit heap with **no** position tracking. Legal because the
/// simulator only ever re-keys the minimum, never an arbitrary actor, so the `pos` array in
/// variant B buys nothing and costs two writes per swap.
struct DAry<const D: usize> {
    nodes: Vec<(u64, u64, u32)>,
}

impl<const D: usize> DAry<D> {
    fn new(cap: u32) -> Self {
        DAry { nodes: Vec::with_capacity(cap as usize) }
    }
    fn less(a: (u64, u64, u32), b: (u64, u64, u32)) -> bool {
        (a.0, a.1) < (b.0, b.1)
    }
    fn push(&mut self, time_ns: u64, seq: u64, actor: u32) {
        self.nodes.push((time_ns, seq, actor));
        let mut i = self.nodes.len() - 1;
        while i > 0 {
            let parent = (i - 1) / D;
            if Self::less(self.nodes[i], self.nodes[parent]) {
                self.nodes.swap(i, parent);
                i = parent;
            } else {
                break;
            }
        }
    }
    fn peek(&self) -> (u64, u64, u32) {
        self.nodes[0]
    }
    fn replace_min(&mut self, time_ns: u64, seq: u64) {
        self.nodes[0].0 = time_ns;
        self.nodes[0].1 = seq;
        let n = self.nodes.len();
        let mut i = 0usize;
        loop {
            let first = D * i + 1;
            if first >= n {
                break;
            }
            let mut best = first;
            for c in (first + 1)..(first + D).min(n) {
                if Self::less(self.nodes[c], self.nodes[best]) {
                    best = c;
                }
            }
            if Self::less(self.nodes[best], self.nodes[i]) {
                self.nodes.swap(i, best);
                i = best;
            } else {
                break;
            }
        }
    }
}

fn dary<const D: usize>(actors: u32, iters: u64, seed: u64) -> (f64, u64) {
    let mut rng = Rng(seed);
    let mut heap: DAry<D> = DAry::new(actors);
    let mut seq = 0u64;
    for a in 0..actors {
        seq += 1;
        heap.push(rng.interval(), seq, a);
    }
    let start = Instant::now();
    let mut sink = 0u64;
    for _ in 0..iters {
        let (time_ns, _, actor) = heap.peek();
        sink = sink.wrapping_add(actor as u64);
        seq += 1;
        heap.replace_min(time_ns + rng.interval(), seq);
    }
    (start.elapsed().as_secs_f64() / iters as f64 * 1e9, sink)
}

fn two_level(actors: u32, iters: u64, seed: u64) -> (f64, u64) {
    let mut rng = Rng(seed);
    let mut heap = FourAry::with_actors(actors);
    let mut seq = 0u64;
    for a in 0..actors {
        seq += 1;
        heap.push(rng.interval(), seq, a);
    }
    let start = Instant::now();
    let mut sink = 0u64;
    for _ in 0..iters {
        let (time_ns, _, actor) = heap.peek();
        sink = sink.wrapping_add(actor as u64);
        seq += 1;
        heap.replace_min(time_ns + rng.interval(), seq);
    }
    (start.elapsed().as_secs_f64() / iters as f64 * 1e9, sink)
}

/// Variant C: a flat binary heap holding only one event per actor, to separate the cost of
/// heap *size* from the cost of the heap *implementation*.
fn flat_heap_one_per_actor(actors: u32, iters: u64, seed: u64) -> (f64, u64) {
    flat_heap(actors, 1, iters, seed)
}

fn main() {
    // 6,250 replicas is the target fleet from docs/ARCHITECTURE.md section 1.1;
    // 62,500 is the 10x stretch.
    const ITERS: u64 = 20_000_000;

    println!("Event queue throughput. {ITERS} pop+push cycles per measurement.");
    println!("Budget from docs/ARCHITECTURE.md 1.4: 100 ns/event.\n");

    println!("{:<46} {:>10} {:>14}", "variant", "ns/event", "events/s");
    println!("{}", "-".repeat(72));

    let mut sink = 0u64;

    for &actors in &[6_250u32, 62_500u32] {
        let (ns, s) = flat_heap_one_per_actor(actors, ITERS, 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("std BinaryHeap, pop+push, {actors} entries"), ns, 1e9 / ns);

        let (ns, s) = dary::<2>(actors, ITERS, 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("2-ary replace_min, no pos, {actors} entries"), ns, 1e9 / ns);

        let (ns, s) = dary::<4>(actors, ITERS, 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("4-ary replace_min, no pos, {actors} entries"), ns, 1e9 / ns);

        let (ns, s) = dary::<8>(actors, ITERS, 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("8-ary replace_min, no pos, {actors} entries"), ns, 1e9 / ns);

        let (ns, s) = two_level(actors, ITERS, 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("4-ary + pos tracking, {actors} entries"), ns, 1e9 / ns);
        println!();
    }

    // The case the plan is worried about: millions of live future events.
    for &(actors, per) in &[(6_250u32, 32u32), (6_250, 272)] {
        let total = actors as u64 * per as u64;
        let (ns, s) = flat_heap(actors, per, ITERS.min(10_000_000), 0x2545F4914F6CDD1D);
        sink ^= s;
        println!("{:<46} {:>10.1} {:>14.0}",
                 format!("flat binary heap, {total} entries"), ns, 1e9 / ns);
    }

    println!("\n(checksum {sink}, printed so nothing is optimised away)");
}
