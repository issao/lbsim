//! The replica engine: one continuously batched inference replica, advanced one step at a time.
//!
//! This is the physics owner of `docs/ARCHITECTURE.md` section 10.8: it knows how a replica admits,
//! prefills, decodes and retires, and nothing about routers, retries, clients or clocks beyond the
//! `now` it is handed. The event loop in `sim-leaf` decides *when* a replica steps and what to do with
//! what it finished; the replica decides what a step *is*. Keeping the two apart is what lets the
//! loop be split along the Leaf seam later without touching a single arithmetic expression here.
//!
//! One replica step is one engine iteration, as in continuous batching: admit from the queue if
//! there is room, do a bounded amount of prefill work, emit one token for every decoding sequence,
//! retire whatever finished. Batch composition therefore changes constantly.
//!
//! **Deviation worth naming.** `docs/ARCHITECTURE.md` section 3 advances a replica by solving a
//! closed form over a whole epoch rather than stepping. That matters when long decode runs dominate,
//! and it needs the KV growth term, which today's scope excludes. The result itself is not in doubt:
//! `bench/validate_epochs.py` proves the closed form exactly equivalent to per-step iteration,
//! including the compute branch and speculation. Stepping here is the simple thing that is correct
//! at today's scale, and the epoch advance lands with the KV model.

use sim_core::Nanos;
use sim_physics::CostModel;
use sim_scenario::Scenario;
use sim_workload::Request;
use std::collections::{BTreeMap, VecDeque};

pub use sim_workload::PrefixTree;

/// The tree of a run with no prefix model. Nothing indexes it, since every request then carries
/// node 0, and `step` needs something to hand `step_with_prefixes`.
const NO_TREE: PrefixTree = PrefixTree::empty();

pub mod trace;
use sim_core::scheduling::{FifoChunked, SchedulingPolicy, SeqView, StepView};
use trace::{ResourceSnapshot, Tracer};

struct Seq {
    req: Request,
    prefill_left: u32,
    output_left: u32,
    admitted_at: Nanos,
    first_token_at: Nanos,
    last_token_at: Nanos,
    max_itl: Nanos,
    itl_sum: Nanos,
    itl_count: u32,
    /// Where the context lives. Anything but `Hbm` means re-admission pays the copy back, not prefill.
    tier: Tier,
    /// Fractional tokens owed by speculative decoding: each step adds the expected tokens per step and
    /// the integer part is emitted, so the long-run rate matches the formula with no random draw.
    spec_credit: f64,
}

impl Seq {
    /// Tokens this sequence holds in the cache: its prompt is charged whole at admission and every
    /// decode step adds one, so it is the prompt plus what has been generated, whatever the prefill
    /// progress. The retire and remove paths subtract exactly this.
    fn resident(&self) -> u64 {
        self.req.prompt as u64 + (self.req.output - self.output_left) as u64
    }
}

/// A finished turn whose context stays resident for the session's next turn. Keyed by that turn's
/// request id, which the loop allocates when it schedules the turn, so admission can find it.
struct Parked {
    id: u64,
    tokens: u64,
    deadline: Nanos,
    parked_at: Nanos,
    tier: Tier,
}

/// Where a sequence's key-value context is: section 7.2's tier tag. A dropped context has no tag
/// because it has no owner left; that is the recompute path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Hbm,
    Dram,
    Ssd,
}

/// The cluster's memory tiers and the fabric they share (architecture section 7.2). DRAM and SSD are
/// pooled at cluster scope because a replica pays a network transfer to reach either wherever the
/// bytes are, so per-host placement is not worth modelling. Migrations debit one shared bandwidth
/// container: a transfer starts when the container is free and runs at the slower of its tier and
/// the fabric, so concurrent migrations queue and contention emerges rather than being assumed. The
/// cost is O(1) per migration.
///
/// With no pools and no fabric this is exactly the per-replica DRAM model that came before it: the
/// decision falls back to each replica's own `dram_capacity_tokens`, and the transfer runs at
/// `swap_gbps`. Every golden run before tiering is byte-identical through this path.
#[derive(Clone, Debug)]
pub struct Tiers {
    /// Cluster DRAM pool in tokens; zero means the per-replica cap decides instead.
    dram_cap: u64,
    /// Cluster SSD pool in tokens; zero means there is no SSD tier.
    ssd_cap: u64,
    dram_used: u64,
    ssd_used: u64,
    /// When the shared fabric frees up. Only advanced when there is a fabric to contend for.
    fabric_busy_until: Nanos,
    /// Transfer time debited so far, by tier, charged when the migration is scheduled. The loop
    /// differences these per sample window for the tier bandwidth gauges.
    dram_busy_ns: Nanos,
    ssd_busy_ns: Nanos,
}

impl Tiers {
    pub fn new(sc: &Scenario) -> Tiers {
        Tiers {
            dram_cap: sc.dram_pool_tokens as u64,
            ssd_cap: sc.ssd_pool_tokens as u64,
            dram_used: 0,
            ssd_used: 0,
            fabric_busy_until: 0,
            dram_busy_ns: 0,
            ssd_busy_ns: 0,
        }
    }

    /// Where a context of `tokens` evicted under `policy` goes: the migration policy of this unit,
    /// fixed. Down as far as the pools allow: DRAM while it has room, then SSD while it has room,
    /// then nowhere, which is a drop and a recompute. `None` is the drop. Without a DRAM pool the
    /// per-replica cap decides as it always did, and with `swap_to_dram` there it never refuses.
    fn place(&self, policy: Policy, tokens: u64, replica_dram: u64, replica_cap: u64) -> Option<Tier> {
        let dram = match policy {
            Policy::Recompute | Policy::Never => return None,
            _ if self.dram_cap > 0 => self.dram_used + tokens <= self.dram_cap,
            Policy::Swap => true,
            Policy::SwapElseRecompute => replica_dram + tokens <= replica_cap,
        };
        if dram {
            Some(Tier::Dram)
        } else if self.ssd_cap > 0 && self.ssd_used + tokens <= self.ssd_cap {
            Some(Tier::Ssd)
        } else {
            None
        }
    }

    /// Debit `tokens` of `tier` to the pool and the fabric at `now`, one direction. Returns the wall
    /// time until the transfer is done, queueing behind the fabric included: what the step is charged.
    fn transfer(&mut self, cost: &CostModel, tier: Tier, tokens: u64, now: Nanos) -> Nanos {
        let (dur, busy) = match tier {
            Tier::Hbm => return 0,
            Tier::Dram => (cost.swap_ns(tokens), &mut self.dram_busy_ns),
            Tier::Ssd => (cost.ssd_ns(tokens), &mut self.ssd_busy_ns),
        };
        *busy += dur;
        if cost.fabric_gbps <= 0.0 {
            return dur;
        }
        let start = now.max(self.fabric_busy_until);
        self.fabric_busy_until = start + dur;
        self.fabric_busy_until - now
    }

    fn hold(&mut self, tier: Tier, tokens: u64) {
        match tier {
            Tier::Hbm => {}
            Tier::Dram => self.dram_used += tokens,
            Tier::Ssd => self.ssd_used += tokens,
        }
    }

    fn release(&mut self, tier: Tier, tokens: u64) {
        match tier {
            Tier::Hbm => {}
            Tier::Dram => self.dram_used = self.dram_used.saturating_sub(tokens),
            Tier::Ssd => self.ssd_used = self.ssd_used.saturating_sub(tokens),
        }
    }

    /// Give the pools back what `r` released outside a step: a timed-out or crashed context whose
    /// bytes were parked in a tier. The loop calls this after every `remove` and `crash`.
    pub fn reclaim(&mut self, r: &mut Replica) {
        self.release(Tier::Dram, std::mem::take(&mut r.released_dram));
        self.release(Tier::Ssd, std::mem::take(&mut r.released_ssd));
    }

    pub fn dram_used(&self) -> u64 {
        self.dram_used
    }
    pub fn ssd_used(&self) -> u64 {
        self.ssd_used
    }
    /// Transfer time debited to each tier so far, DRAM then SSD.
    pub fn busy_ns(&self) -> (Nanos, Nanos) {
        (self.dram_busy_ns, self.ssd_busy_ns)
    }
    /// When the shared fabric is next free; `now` or earlier means idle.
    pub fn fabric_busy_until(&self) -> Nanos {
        self.fabric_busy_until
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Policy {
    Never,
    Recompute,
    Swap,
    SwapElseRecompute,
}

impl Policy {
    fn parse(name: &str) -> Policy {
        match name {
            "recompute" => Policy::Recompute,
            "swap_to_dram" => Policy::Swap,
            "swap_else_recompute" => Policy::SwapElseRecompute,
            _ => Policy::Never,
        }
    }
}

pub struct Replica {
    queue: VecDeque<Request>,
    running: Vec<Seq>,
    /// Running sequences evicted under pressure, head first. They re-enter ahead of the queue,
    /// because they were admitted before anything in it.
    preempted: VecDeque<Seq>,
    parked: Vec<Parked>,
    queued_tokens: u64,
    /// Resident key-value tokens: for every running sequence, its prompt plus what it has generated,
    /// and for every parked session its whole context.
    /// This is the real capacity constraint, and it is denominated in tokens rather than requests.
    kv_tokens: u64,
    /// Context swapped out to host memory, in tokens, and to the SSD tier.
    dram_tokens: u64,
    ssd_tokens: u64,
    /// Tiered context this replica let go of outside a step (a timeout, a crash), not yet handed
    /// back to the cluster pools. `Tiers::reclaim` drains it.
    released_dram: u64,
    released_ssd: u64,
    next_step_at: Nanos,
    scheduled: bool,
    last_step_ns: Nanos,
    /// The step clock: nanoseconds spent inside a step since the run began, and of those the part the
    /// cost model priced as compute. Both are charged in full when a step starts, and `[step_start,
    /// step_end)` remembers the step so a reader at an instant inside it can subtract what has not
    /// happened yet. That is what lets a sample window cut a step exactly instead of rounding it to
    /// one side, so utilization never exceeds one by construction.
    busy_total_ns: Nanos,
    compute_total_ns: Nanos,
    step_start: Nanos,
    step_end: Nanos,
    /// Compute part of the step in flight, for the proportional clip.
    step_compute_ns: Nanos,
    completed: u64,
    preemptions: u64,
    /// See `last_view_len`.
    last_view_len: usize,
    /// Records what happens to the sequences the loop asked to trace; inert otherwise.
    tracer: Tracer,
    /// Fraction of modelled speed; 1 is healthy, 0.3 is a gray failure, 0 is a hang. Nothing but
    /// `last_step_ns` in telemetry betrays it, which is the point of modelling it.
    speed: f64,
    /// Crashed: holds nothing, refuses everything, and telemetry says so after the delay.
    down: bool,
    /// Prefix cache, `docs/ARCHITECTURE.md` section 7.3: node to last use. Residency is prefix-closed,
    /// since a node is inserted with its ancestors, so the deepest resident node on a request's path
    /// is the whole hit. A `BTreeMap` rather than a hash map so eviction order is deterministic.
    prefix_cache: BTreeMap<u64, Nanos>,
    /// Tokens the resident prefix nodes add up to, against `prefix_cache_tokens`.
    prefix_used: u64,
    /// Residency changes since the loop last asked, so it can keep a fleet-wide index.
    prefix_inserted: Vec<u64>,
    prefix_evicted: Vec<u64>,
    /// Running totals of admitted prompt tokens and of the part a prefix hit or parked context
    /// spared; the loop differences them per sample window.
    prompt_total: u64,
    hit_total: u64,
    /// Cumulative nanoseconds to first token, and the count of sequences it covers, over the whole
    /// run. Same definition as the run-wide TTFT histogram: `first_token_at - req.arrived_at`. What
    /// the per-replica TTFT mean on the wire is built from; a per-replica histogram would not be
    /// cheap at scale, but two running totals are.
    ttft_sum_ns: u64,
    ttft_count: u64,
}

impl Default for Replica {
    fn default() -> Self {
        Replica {
            queue: VecDeque::new(),
            running: Vec::new(),
            preempted: VecDeque::new(),
            parked: Vec::new(),
            queued_tokens: 0,
            kv_tokens: 0,
            dram_tokens: 0,
            ssd_tokens: 0,
            released_dram: 0,
            released_ssd: 0,
            next_step_at: 0,
            scheduled: false,
            last_step_ns: 0,
            busy_total_ns: 0,
            compute_total_ns: 0,
            step_start: 0,
            step_end: 0,
            step_compute_ns: 0,
            completed: 0,
            preemptions: 0,
            last_view_len: 0,
            tracer: Tracer::default(),
            speed: 1.0,
            down: false,
            prefix_cache: BTreeMap::new(),
            prefix_used: 0,
            prefix_inserted: Vec::new(),
            prefix_evicted: Vec::new(),
            prompt_total: 0,
            hit_total: 0,
            ttft_sum_ns: 0,
            ttft_count: 0,
        }
    }
}

/// A sequence that emitted its last token this step, with what the record needs. The SLO verdict is
/// deliberately not here: the thresholds are the loop's business, the timings are the replica's.
pub struct FinishedSeq {
    pub req: Request,
    pub admitted_at: Nanos,
    pub first_token_at: Nanos,
    pub max_itl: Nanos,
    pub mean_itl: Nanos,
}

/// What one step did. `idle` means the replica has nothing left to run and will not step again until
/// something is enqueued, so the loop must not schedule a follow-up step.
pub struct StepOutcome {
    pub step_ns: Nanos,
    pub token_at: Nanos,
    pub finished: Vec<FinishedSeq>,
    pub idle: bool,
    /// Contexts evicted from the cache this step, running or parked.
    pub preempted: u32,
}

impl Replica {
    /// Queue a request, or hand it back unchanged when the queue is full so the caller can shed it
    /// before it consumes any device time.
    pub fn enqueue(&mut self, req: Request, max_queue: usize) -> Result<(), Request> {
        if self.queue.len() >= max_queue {
            return Err(req);
        }
        self.queued_tokens += req.prompt as u64;
        self.queue.push_back(req);
        Ok(())
    }

    /// Mark the replica as due to step at `now`. True when the caller must schedule that step; false
    /// when one is already pending, so a replica never has two step events in flight.
    pub fn wake(&mut self, now: Nanos) -> bool {
        if !self.scheduled {
            self.scheduled = true;
            self.next_step_at = now;
            true
        } else {
            false
        }
    }

    /// Keep a finished turn's context resident for the session's next turn, request `next_id`, which
    /// the loop will enqueue here after the think time. The tokens were released when the turn
    /// retired; this takes them back, so between turns the session costs what it did while running.
    pub fn park(&mut self, next_id: u64, tokens: u64, deadline: Nanos, now: Nanos) {
        self.kv_tokens += tokens;
        self.parked.push(Parked { id: next_id, tokens, deadline, parked_at: now, tier: Tier::Hbm });
    }

    /// The head of the queue as the scheduler sees it: at most `max_batch` entries, never the whole
    /// queue (see `sim_policy::scheduling`).
    fn queued_view(&self, sc: &Scenario, out: &mut Vec<SeqView>) {
        out.clear();
        out.extend(self.queue.iter().take(sc.max_batch).map(|req| SeqView {
            id: req.id,
            class: req.class,
            deadline: req.deadline,
            arrived_at: req.arrived_at,
            admitted_at: 0,
            prompt_tokens: req.prompt,
            prefill_left: req.prompt,
            resident_tokens: 0,
            queued: true,
        }));
    }

    fn running_view(&self, out: &mut Vec<SeqView>) {
        out.clear();
        out.extend(self.running.iter().map(|s| SeqView {
            id: s.req.id,
            class: s.req.class,
            deadline: s.req.deadline,
            arrived_at: s.req.arrived_at,
            admitted_at: s.admitted_at,
            prompt_tokens: s.req.prompt,
            prefill_left: s.prefill_left,
            resident_tokens: s.resident(),
            queued: false,
        }));
    }

    fn view<'a>(
        &self,
        sc: &Scenario,
        now: Nanos,
        queued: &'a [SeqView],
        running: &'a [SeqView],
    ) -> StepView<'a> {
        StepView {
            now,
            queued,
            running,
            kv_tokens: self.kv_tokens,
            kv_capacity: sc.kv_capacity_tokens as u64,
            max_batch: sc.max_batch,
            step_token_budget: sc.step_token_budget,
            prefill_tokens_per_s: sc.prefill_tokens_per_s,
        }
    }

    /// Evict one context under `policy`, the victim chosen by `sched`. Parked context goes before a
    /// running sequence, since dropping idle context stalls nobody, and the parked entry for `keep`
    /// is exempt, because it is the one the request at the head of the queue is about to reuse. A
    /// recompute drops the context; a swap moves it down a tier and charges the transfer to this
    /// step. False when there is nothing left to evict.
    #[allow(clippy::too_many_arguments)]
    fn evict(
        &mut self,
        sc: &Scenario,
        now: Nanos,
        sched: &mut dyn SchedulingPolicy,
        policy: Policy,
        keep: Option<u64>,
        cost: &CostModel,
        tiers: &mut Tiers,
        dram_cap: u64,
        extra_ns: &mut Nanos,
        running_too: bool,
    ) -> bool {
        // The views are built here rather than kept across the step because an eviction changes
        // them, and evictions are rare: this costs O(batch + queue head) only under pressure.
        let mut queued = Vec::new();
        let mut running = Vec::new();
        self.queued_view(sc, &mut queued);
        self.running_view(&mut running);
        let view = self.view(sc, now, &queued, &running);
        // Eligible parked contexts and where each sits in `parked`.
        let mut slots = Vec::new();
        let mut candidates = Vec::new();
        for (i, p) in self.parked.iter().enumerate() {
            if p.tier == Tier::Hbm && Some(p.id) != keep {
                slots.push(i);
                candidates.push(SeqView {
                    id: p.id,
                    class: 0,
                    deadline: p.deadline,
                    arrived_at: p.parked_at,
                    admitted_at: p.parked_at,
                    prompt_tokens: p.tokens.min(u32::MAX as u64) as u32,
                    prefill_left: 0,
                    resident_tokens: p.tokens,
                    queued: false,
                });
            }
        }
        let parked = if candidates.is_empty() {
            None
        } else {
            sched.victim(&view, &candidates).filter(|&i| i < slots.len()).map(|i| slots[i])
        };
        if let Some(i) = parked {
            let tokens = self.parked[i].tokens;
            self.kv_tokens = self.kv_tokens.saturating_sub(tokens);
            if let Some(tier) = tiers.place(policy, tokens, self.dram_tokens, dram_cap) {
                self.parked[i].tier = tier;
                self.hold(tiers, tier, tokens);
                *extra_ns += tiers.transfer(cost, tier, tokens, now);
            } else {
                self.parked.swap_remove(i);
            }
            self.preemptions += 1;
            return true;
        }
        if !running_too || running.is_empty() {
            return false;
        }
        let Some(i) = sched.victim(&view, &running).filter(|&i| i < running.len()) else {
            return false;
        };
        let mut s = self.running.swap_remove(i);
        let tokens = s.resident();
        self.kv_tokens = self.kv_tokens.saturating_sub(tokens);
        if let Some(tier) = tiers.place(policy, tokens, self.dram_tokens, dram_cap) {
            s.tier = tier;
            self.hold(tiers, tier, tokens);
            *extra_ns += tiers.transfer(cost, tier, tokens, now);
        } else {
            // Everything generated so far becomes prompt to compute again; the prefill done this
            // step, if any, is wasted work the device still did.
            s.prefill_left = tokens as u32;
        }
        self.preempted.push_front(s);
        self.preemptions += 1;
        true
    }

    /// Deepest resident node on `node`'s path, as the tokens it spares: the hit length the request
    /// would see here. Zero for node 0 and for a cold cache.
    pub fn prefix_hit(&self, tree: &PrefixTree, node: u64) -> u32 {
        tree.path_tokens(self.prefix_hit_node(tree, node))
    }

    fn prefix_hit_node(&self, tree: &PrefixTree, node: u64) -> u64 {
        let mut n = node;
        while n != 0 && !self.prefix_cache.contains_key(&n) {
            n = tree.parent(n);
        }
        n
    }

    /// Mark `node` and its ancestors used at `now`, inserting whichever were not resident, then
    /// evict least recently used nodes until the budget holds. Ties go to the newest node id,
    /// because an ancestor is touched whenever a descendant is and so never has the older use; the
    /// deepest node of a chain therefore leaves first and residency stays prefix-closed.
    fn prefix_touch(&mut self, tree: &PrefixTree, node: u64, now: Nanos, cap: u64) {
        let mut n = node;
        while n != 0 {
            if self.prefix_cache.insert(n, now).is_none() {
                self.prefix_used += tree.tokens(n) as u64;
                self.prefix_inserted.push(n);
            }
            n = tree.parent(n);
        }
        while self.prefix_used > cap {
            let Some((&victim, _)) =
                self.prefix_cache.iter().min_by_key(|(id, &t)| (t, std::cmp::Reverse(**id)))
            else {
                break;
            };
            self.prefix_cache.remove(&victim);
            self.prefix_used = self.prefix_used.saturating_sub(tree.tokens(victim) as u64);
            self.prefix_evicted.push(victim);
        }
    }

    /// Node ids inserted into and evicted from the prefix cache since the last call, in that
    /// order of events within each list. A node that came and went in between appears in both.
    pub fn drain_prefix_changes(&mut self) -> (Vec<u64>, Vec<u64>) {
        (std::mem::take(&mut self.prefix_inserted), std::mem::take(&mut self.prefix_evicted))
    }

    /// Tokens the prefix cache holds.
    pub fn prefix_cache_tokens(&self) -> u64 {
        self.prefix_used
    }
    /// Prompt tokens admitted so far, and of those the tokens a prefix hit or parked context spared.
    pub fn prompt_tokens_total(&self) -> u64 {
        self.prompt_total
    }
    pub fn prefix_hit_tokens_total(&self) -> u64 {
        self.hit_total
    }

    /// One engine iteration at `now`. `None` when there was nothing to run, in which case the replica
    /// has parked itself and the loop schedules nothing. For a run without a prefix model; the tree
    /// is only consulted for requests that carry a node, so an empty one is exact here.
    pub fn step(&mut self, sc: &Scenario, cost: &CostModel, now: Nanos) -> Option<StepOutcome> {
        self.step_with_prefixes(sc, cost, now, &NO_TREE)
    }

    /// `step`, with the tree the requests' `prefix_node`s index, under the engine's own default
    /// scheduler. Fine for a test driving one replica; the loop resolves the scenario's `scheduling`
    /// name through the policy registry, holds one per replica and calls `step_scheduled` directly.
    pub fn step_with_prefixes(
        &mut self,
        sc: &Scenario,
        cost: &CostModel,
        now: Nanos,
        tree: &PrefixTree,
    ) -> Option<StepOutcome> {
        let mut sched = FifoChunked::new(&sc.preemption_victim);
        self.step_scheduled(sc, cost, now, tree, &mut sched)
    }

    /// `step_tiered` with tiers built fresh from the scenario, which is exact for a scenario without
    /// cluster pools. A caller driving one replica against a pooled scenario holds the `Tiers` and
    /// calls `step_tiered`; the loop always does.
    pub fn step_scheduled(
        &mut self,
        sc: &Scenario,
        cost: &CostModel,
        now: Nanos,
        tree: &PrefixTree,
        sched: &mut dyn SchedulingPolicy,
    ) -> Option<StepOutcome> {
        self.step_tiered(sc, cost, now, tree, sched, &mut Tiers::new(sc))
    }

    /// Entries of the queue the scheduler was shown at the last step: at most `max_batch`, whatever
    /// the queue holds. Exposed so a test can prove a policy never receives the whole queue.
    pub fn last_view_len(&self) -> usize {
        self.last_view_len
    }

    /// `step_with_prefixes`, with the replica's scheduler and the cluster's memory tiers. The policy
    /// is consulted at exactly four points: the admission order, the prefill budget and its order,
    /// and each victim. The tiers decide where a victim's context goes and what the move costs.
    pub fn step_tiered(
        &mut self,
        sc: &Scenario,
        cost: &CostModel,
        now: Nanos,
        tree: &PrefixTree,
        sched: &mut dyn SchedulingPolicy,
        tiers: &mut Tiers,
    ) -> Option<StepOutcome> {
        let r = self;
        tiers.reclaim(r);
        let prefix_cap = sc.prefix_cache_tokens as u64;
        // A crashed replica holds nothing and a hung one never finishes a step. Either way there is
        // no follow-up to schedule, so this reads as idle; what it holds waits for the client timeout.
        if r.down || r.speed <= 0.0 {
            r.scheduled = false;
            return None;
        }
        let policy = Policy::parse(&sc.preemption);
        let dram_cap = sc.dram_capacity_tokens() as u64;
        // Transfers charged to this step: the device is busy copying context for as long as they take.
        let mut extra_ns: Nanos = 0;
        let mut preempted = 0u32;
        // Admit while there is room. The replica is itself a scheduler, so this is the second
        // scheduling layer and it can disagree with the router.
        //
        // Two limits, and which one binds is the point: a sequence-count cap, and the
        // key-value token budget. A 24,000-token prompt consumes what eight chat turns
        // consume, so a queue of long requests blocks admission that a request count would
        // have allowed. A blocked request waits, which is what produces the queueing this
        // scenario is about; a preemption policy only makes room by evicting idle context here,
        // never a running sequence, which would trade one victim for another every step.
        let kv_cap = sc.kv_capacity_tokens as u64;
        // The scheduler orders the head of the queue, at most `max_batch` entries, once per step.
        // `order` holds queue positions still to try, head first; taking one shifts the positions
        // behind it down by one. FIFO returns the identity, and then this loop is exactly the
        // pop-front loop it replaced.
        let mut queued = Vec::new();
        let mut running = Vec::new();
        r.queued_view(sc, &mut queued);
        r.running_view(&mut running);
        r.last_view_len = queued.len();
        let mut order: VecDeque<usize> = VecDeque::new();
        {
            let mut seen = vec![false; queued.len()];
            for i in sched.admit_order(&r.view(sc, now, &queued, &running)) {
                if i < seen.len() && !seen[i] {
                    seen[i] = true;
                    order.push_back(i);
                }
            }
        }
        while r.running.len() < sc.max_batch {
            // Evicted sequences first, then the queue in the scheduler's order. A queued session
            // turn whose context is still resident costs only its new tokens.
            let next_pos = order.front().copied();
            let next_cost = if let Some(s) = r.preempted.front() {
                s.resident()
            } else if let Some(req) = next_pos.map(|i| &r.queue[i]) {
                match r.parked.iter().find(|p| p.id == req.id) {
                    Some(p) if p.tier == Tier::Hbm => (req.prompt as u64).saturating_sub(p.tokens),
                    _ => req.prompt as u64,
                }
            } else {
                break;
            };
            if r.kv_tokens + next_cost > kv_cap {
                let keep = next_pos.map(|i| r.queue[i].id).or_else(|| r.queue.front().map(|q| q.id));
                if policy != Policy::Never
                    && r.evict(sc, now, sched, policy, keep, cost, tiers, dram_cap, &mut extra_ns, false)
                {
                    preempted += 1;
                    continue;
                }
                if !r.running.is_empty() {
                    break;
                }
            }
            if let Some(mut s) = r.preempted.pop_front() {
                let tokens = s.resident();
                if s.tier != Tier::Hbm {
                    r.release(tiers, s.tier, tokens);
                    extra_ns += tiers.transfer(cost, s.tier, tokens, now);
                    s.tier = Tier::Hbm;
                }
                r.kv_tokens += tokens;
                r.running.push(s);
                continue;
            }
            let Some(pos) = order.pop_front() else { break };
            for o in order.iter_mut() {
                if *o > pos {
                    *o -= 1;
                }
            }
            match r.queue.remove(pos) {
                Some(req) => {
                    r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
                    // Parked context and a prefix hit both spare prefill over the same leading
                    // tokens, so the larger wins and they never add. The key-value charge is the
                    // whole prompt either way: shared blocks are not modelled, a hit spares the
                    // compute and not the memory.
                    let hit_node =
                        if prefix_cap > 0 { r.prefix_hit_node(tree, req.prefix_node) } else { 0 };
                    let hit = tree.path_tokens(hit_node).min(req.prompt);
                    let mut spared = hit;
                    if let Some(i) = r.parked.iter().position(|p| p.id == req.id) {
                        let p = r.parked.swap_remove(i);
                        let reused = p.tokens.min(req.prompt as u64);
                        if p.tier != Tier::Hbm {
                            r.release(tiers, p.tier, p.tokens);
                            extra_ns += tiers.transfer(cost, p.tier, p.tokens, now);
                            r.kv_tokens += req.prompt as u64;
                        } else {
                            r.kv_tokens += req.prompt as u64 - reused;
                        }
                        spared = spared.max(reused as u32);
                    } else {
                        r.kv_tokens += req.prompt as u64;
                    }
                    let prefill_left = req.prompt - spared;
                    // A hit refreshes what it reused; the request's own deeper nodes become
                    // resident only once their prefill has actually run.
                    if hit_node != 0 {
                        r.prefix_touch(tree, hit_node, now, prefix_cap);
                    }
                    r.prompt_total += req.prompt as u64;
                    r.hit_total += spared as u64;
                    r.running.push(Seq {
                        prefill_left,
                        output_left: req.output,
                        admitted_at: now,
                        first_token_at: 0,
                        last_token_at: 0,
                        max_itl: 0,
                        itl_sum: 0,
                        itl_count: 0,
                        tier: Tier::Hbm,
                        spec_credit: 0.0,
                        req,
                    });
                    r.tracer.admitted(r.running[r.running.len() - 1].req.id);
                }
                None => break,
            }
        }
        if r.running.is_empty() {
            r.scheduled = false;
            return None;
        }

        // Chunked prefill: a bounded token budget per step, taken in admission order. This
        // is what stops one long prompt from inserting a multi-second stall into everyone
        // else's token stream. The scheduler sets the budget from the batch it just built.
        r.queued_view(sc, &mut queued);
        r.running_view(&mut running);
        let view = r.view(sc, now, &queued, &running);
        let mut budget = sched.prefill_budget(&view);
        let order = sched.prefill_order(&view);
        let mut prefill_tokens = 0u32;
        let chunk = |s: &mut Seq, tracer: &mut Tracer, budget: &mut u32, prefill_tokens: &mut u32| {
            if s.prefill_left > 0 {
                let take = s.prefill_left.min(*budget);
                s.prefill_left -= take;
                *budget -= take;
                *prefill_tokens += take;
                tracer.prefill_chunk(s.req.id, take);
            }
        };
        match order {
            None => {
                for s in r.running.iter_mut() {
                    if budget == 0 {
                        break;
                    }
                    chunk(s, &mut r.tracer, &mut budget, &mut prefill_tokens);
                }
            }
            Some(order) => {
                let mut seen = vec![false; r.running.len()];
                for i in order {
                    if budget == 0 {
                        break;
                    }
                    if i < seen.len() && !seen[i] {
                        seen[i] = true;
                        chunk(&mut r.running[i], &mut r.tracer, &mut budget, &mut prefill_tokens);
                    }
                }
            }
        }
        let mut decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();

        // A decode step grows every decoding sequence by one token. When that would not fit, the
        // policy evicts until it does: parked context first, then running sequences by the victim
        // rule, and a sequence evicted here re-enters at the head of admission next step. A lone
        // sequence is never evicted: admission let it in over the cap, and evicting it would only
        // re-admit it next step. The queue head's own parked context is exempt here as at
        // admission: swapped out now, it would be swapped straight back when its turn comes, two
        // transfers for nothing.
        if policy != Policy::Never {
            let keep = r.queue.front().map(|q| q.id);
            while r.kv_tokens + decoding as u64 > kv_cap
                && r.evict(
                    sc,
                    now,
                    sched,
                    policy,
                    keep,
                    cost,
                    tiers,
                    dram_cap,
                    &mut extra_ns,
                    r.running.len() > 1,
                )
            {
                preempted += 1;
                decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();
            }
        }

        // Step time comes from the cost model in sim-physics, the one place that formula
        // lives. Prefill and decode contend for one device, which is why a big prefill shows
        // up in everyone's inter-token latency.
        let (compute, modelled) = cost.step_split(decoding, r.kv_tokens, prefill_tokens);
        let modelled = modelled + extra_ns;
        // A slowed replica takes 1/speed of the modelled time. The healthy case skips the float trip
        // so a run with no failures is byte-identical to one before failures existed. Swap transfers
        // are busy time but not compute, the link is what they wait on.
        let (step_ns, compute_ns) = if r.speed == 1.0 {
            (modelled, compute)
        } else {
            ((modelled as f64 / r.speed) as Nanos, (compute as f64 / r.speed) as Nanos)
        };
        let token_at = now + step_ns;
        r.last_step_ns = step_ns;
        r.busy_total_ns += step_ns;
        r.compute_total_ns += compute_ns;
        r.step_start = now;
        r.step_end = token_at;
        r.step_compute_ns = compute_ns;
        r.tracer.snapshot(ResourceSnapshot { start: now, end: token_at, batch_size: r.running.len() as u32, running: r.running.len() as u32, queued: r.queue.len() as u32, kv_tokens: r.kv_tokens, decoding: decoding as u32, prefill_tokens, step_ns });

        // With speculation each sequence advances by the expected tokens per step, carried as a
        // fractional credit so the long-run rate is exact; off, the credit is exactly 1.0 a step and
        // every existing run is unchanged. The tokens of one step arrive together, so the gap a user
        // sees is still the whole step (max ITL) while the mean is per token produced.
        let tokens_per_step = cost.spec_tokens_per_step();
        let mut finished: Vec<usize> = Vec::new();
        let mut generated = 0u64;
        // Prefills that completed this step: their prefixes become resident once the loop below is
        // done with the batch, since the cache and the batch cannot be borrowed together.
        let mut prefilled: Vec<u64> = Vec::new();
        for (idx, s) in r.running.iter_mut().enumerate() {
            if s.prefill_left > 0 {
                continue;
            }
            s.spec_credit += tokens_per_step;
            let emit = s.spec_credit.floor();
            s.spec_credit -= emit;
            let produced = (emit as u32).min(s.output_left);
            if s.first_token_at == 0 {
                s.first_token_at = token_at;
                if prefix_cap > 0 && s.req.prefix_node != 0 {
                    prefilled.push(s.req.prefix_node);
                }
                r.ttft_sum_ns += token_at - s.req.arrived_at;
                r.ttft_count += 1;
            } else {
                let gap = token_at - s.last_token_at;
                s.max_itl = s.max_itl.max(gap);
                s.itl_sum += gap;
                s.itl_count += produced.max(1);
            }
            s.last_token_at = token_at;
            r.tracer.decode_step(s.req.id);
            s.output_left -= produced;
            generated += produced as u64;
            if s.output_left == 0 {
                finished.push(idx);
            }
        }
        r.kv_tokens += generated;
        for node in prefilled {
            r.prefix_touch(tree, node, token_at, prefix_cap);
        }

        let mut retired: Vec<FinishedSeq> = Vec::with_capacity(finished.len());
        for idx in finished.iter().rev() {
            let s = r.running.swap_remove(*idx);
            r.kv_tokens = r
                .kv_tokens
                .saturating_sub(s.req.prompt as u64 + s.req.output as u64);
            r.completed += 1;
            r.tracer.retired(s.req.id);
            let mean_itl = if s.itl_count > 0 { s.itl_sum / s.itl_count as Nanos } else { 0 };
            retired.push(FinishedSeq {
                req: s.req,
                admitted_at: s.admitted_at,
                first_token_at: s.first_token_at,
                max_itl: s.max_itl,
                mean_itl,
            });
        }

        let idle = r.running.is_empty() && r.queue.is_empty() && r.preempted.is_empty();
        if idle {
            r.scheduled = false;
        } else {
            r.next_step_at = token_at;
        }
        Some(StepOutcome { step_ns, token_at, finished: retired, idle, preempted })
    }

    /// Pull a request out wherever it is. The bool says whether it was running, because where it
    /// died decides how expensive the failure was. Any context parked for it goes too: the turn
    /// that would have reused it is not coming.
    pub fn remove(&mut self, id: u64) -> Option<(Request, bool)> {
        let r = self;
        let mut victim: Option<(Request, bool)> = None;
        if let Some(pos) = r.queue.iter().position(|x| x.id == id) {
            let req = r.queue.remove(pos).unwrap();
            r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
            victim = Some((req, false));
        } else if let Some(pos) = r.running.iter().position(|s| s.req.id == id) {
            let s = r.running.swap_remove(pos);
            r.kv_tokens = r.kv_tokens.saturating_sub(s.resident());
            victim = Some((s.req, true));
        } else if let Some(pos) = r.preempted.iter().position(|s| s.req.id == id) {
            let s = r.preempted.remove(pos).unwrap();
            r.let_go(s.tier, s.resident());
            victim = Some((s.req, true));
        }
        if let Some(pos) = r.parked.iter().position(|p| p.id == id) {
            let p = r.parked.swap_remove(pos);
            if p.tier != Tier::Hbm {
                r.let_go(p.tier, p.tokens);
            } else {
                r.kv_tokens = r.kv_tokens.saturating_sub(p.tokens);
            }
        }
        victim
    }

    /// Queued plus running: the load a balancer is trying to even out. An evicted sequence is
    /// admitted work that has not finished, so it counts.
    pub fn load(&self) -> usize {
        self.queue.len() + self.running.len() + self.preempted.len()
    }
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
    pub fn running(&self) -> usize {
        self.running.len()
    }
    /// Tokens of work still owed: queued prompts plus what running sequences have yet to prefill or
    /// emit. The quantity a token-aware router should see, since one long request costs what many
    /// chat turns cost.
    pub fn outstanding_tokens(&self) -> u64 {
        self.queued_tokens
            + self
                .running
                .iter()
                .chain(self.preempted.iter())
                .map(|s| s.prefill_left as u64 + s.output_left as u64)
                .sum::<u64>()
    }
    pub fn kv_tokens(&self) -> u64 {
        self.kv_tokens
    }
    pub fn dram_tokens(&self) -> u64 {
        self.dram_tokens
    }
    pub fn ssd_tokens(&self) -> u64 {
        self.ssd_tokens
    }

    /// Book `tokens` of this replica's context into `tier`, on the replica and in the pool.
    fn hold(&mut self, tiers: &mut Tiers, tier: Tier, tokens: u64) {
        match tier {
            Tier::Hbm => {}
            Tier::Dram => self.dram_tokens += tokens,
            Tier::Ssd => self.ssd_tokens += tokens,
        }
        tiers.hold(tier, tokens);
    }

    /// The reverse of `hold`, at re-admission.
    fn release(&mut self, tiers: &mut Tiers, tier: Tier, tokens: u64) {
        match tier {
            Tier::Hbm => {}
            Tier::Dram => self.dram_tokens = self.dram_tokens.saturating_sub(tokens),
            Tier::Ssd => self.ssd_tokens = self.ssd_tokens.saturating_sub(tokens),
        }
        tiers.release(tier, tokens);
    }

    /// Drop tiered context without a `Tiers` in hand: the pool hears of it at `Tiers::reclaim`.
    fn let_go(&mut self, tier: Tier, tokens: u64) {
        match tier {
            Tier::Hbm => {}
            Tier::Dram => {
                self.dram_tokens = self.dram_tokens.saturating_sub(tokens);
                self.released_dram += tokens;
            }
            Tier::Ssd => {
                self.ssd_tokens = self.ssd_tokens.saturating_sub(tokens);
                self.released_ssd += tokens;
            }
        }
    }
    /// Contexts evicted so far, running or parked.
    pub fn preemptions(&self) -> u64 {
        self.preemptions
    }
    /// Sessions whose context is resident between turns.
    pub fn parked(&self) -> usize {
        self.parked.len()
    }
    pub fn last_step_ns(&self) -> Nanos {
        self.last_step_ns
    }
    pub fn next_step_at(&self) -> Nanos {
        self.next_step_at
    }
    /// Nanoseconds this replica has spent inside a step up to and including `t`. A step in flight at
    /// `t` counts only up to `t`, so the difference between two readings is exactly the busy time of
    /// the window between them, and never more than the window. A crashed or hung replica starts no
    /// step, so it accrues nothing new.
    pub fn busy_ns_through(&self, t: Nanos) -> Nanos {
        self.busy_total_ns - self.overhang(t)
    }
    /// The part of [`Replica::busy_ns_through`] the cost model priced as compute rather than
    /// bandwidth, clipped in proportion within a step in flight.
    pub fn compute_ns_through(&self, t: Nanos) -> Nanos {
        let over = self.overhang(t);
        if over == 0 {
            return self.compute_total_ns;
        }
        let step = self.step_end - self.step_start;
        self.compute_total_ns - (self.step_compute_ns as u128 * over as u128 / step as u128) as Nanos
    }
    /// How much of the step in flight lies past `t`.
    fn overhang(&self, t: Nanos) -> Nanos {
        self.step_end.saturating_sub(t.max(self.step_start))
    }
    pub fn completed(&self) -> u64 {
        self.completed
    }
    /// The loop's handle on what this replica records about traced sequences.
    pub fn tracer_mut(&mut self) -> &mut Tracer {
        &mut self.tracer
    }

    /// Lose everything, at once: queued, running, evicted and parked, in the order they were held,
    /// so the loop can fail each one. The replica refuses work until `recover`.
    pub fn crash(&mut self) -> Vec<Request> {
        let r = self;
        r.down = true;
        r.scheduled = false;
        let mut lost: Vec<Request> = r.queue.drain(..).collect();
        lost.extend(r.running.drain(..).map(|s| s.req));
        lost.extend(r.preempted.drain(..).map(|s| s.req));
        r.parked.clear();
        r.queued_tokens = 0;
        r.kv_tokens = 0;
        r.released_dram += r.dram_tokens;
        r.released_ssd += r.ssd_tokens;
        r.dram_tokens = 0;
        r.ssd_tokens = 0;
        // The cache dies with the device, and the index must hear of every node it held.
        r.prefix_evicted.extend(r.prefix_cache.keys().copied());
        r.prefix_cache.clear();
        r.prefix_used = 0;
        lost
    }
    /// Run at this fraction of modelled speed; 0 hangs. Nothing is announced.
    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
    }
    /// Back to healthy, whatever was wrong. What it holds, if anything, waits for the next wake.
    pub fn recover(&mut self) {
        self.down = false;
        self.speed = 1.0;
    }
    pub fn is_down(&self) -> bool {
        self.down
    }
    pub fn speed(&self) -> f64 {
        self.speed
    }
    /// The replica's state as the engine knows it, `METRIC_REPLICA_STATE`'s number: 1 READY, 2
    /// DEGRADED (slow or hung: `speed < 1.0`), 3 EJECTED (`down`). Down wins over slow because
    /// `crash` does not reset `speed`.
    pub fn state(&self) -> u8 {
        if self.down {
            3
        } else if self.speed < 1.0 {
            2
        } else {
            1
        }
    }
    /// Cumulative nanoseconds to first token across every sequence that has emitted one, this
    /// replica's whole run. Paired with `ttft_count` for a mean; `Window::close` differences
    /// consecutive readings into a per-window mean the same way it does `busy_ns_through`.
    pub fn ttft_sum_ns(&self) -> u64 {
        self.ttft_sum_ns
    }
    pub fn ttft_count(&self) -> u64 {
        self.ttft_count
    }
}

#[cfg(test)]
mod state_tests {
    use super::Replica;

    #[test]
    fn state_is_ready_by_default() {
        assert_eq!(Replica::default().state(), 1);
    }

    #[test]
    fn state_is_degraded_under_a_slow_failure() {
        let mut r = Replica::default();
        r.set_speed(0.3);
        assert_eq!(r.state(), 2);
    }

    #[test]
    fn state_is_degraded_hung() {
        let mut r = Replica::default();
        r.set_speed(0.0);
        assert_eq!(r.state(), 2);
    }

    #[test]
    fn state_is_ejected_after_a_crash() {
        let mut r = Replica::default();
        r.crash();
        assert_eq!(r.state(), 3);
    }

    #[test]
    fn crash_wins_over_a_slow_failure() {
        let mut r = Replica::default();
        r.set_speed(0.3);
        r.crash();
        assert_eq!(r.state(), 3);
    }
}
