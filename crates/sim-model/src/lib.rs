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
use std::collections::VecDeque;

pub mod trace;
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
    /// Context is in host memory rather than the cache: re-admission pays the copy back, not prefill.
    swapped: bool,
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
    swapped: bool,
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

#[derive(Clone, Copy, PartialEq)]
enum Victim {
    Newest,
    LargestKv,
    LatestDeadline,
}

impl Victim {
    fn parse(name: &str) -> Victim {
        match name {
            "largest_kv" => Victim::LargestKv,
            "latest_deadline" => Victim::LatestDeadline,
            _ => Victim::Newest,
        }
    }
    /// The index whose (policy key, index) is largest, so ties resolve to the later entry and the
    /// choice is a pure function of the state.
    fn pick(self, keys: impl Iterator<Item = Option<(Nanos, u64, Nanos)>>) -> Option<usize> {
        let mut best: Option<(u64, usize)> = None;
        for (i, k) in keys.enumerate() {
            let Some((at, tokens, deadline)) = k else { continue };
            let key = match self {
                Victim::Newest => at,
                Victim::LargestKv => tokens,
                Victim::LatestDeadline => deadline,
            };
            if best.map_or(true, |(b, _)| key >= b) {
                best = Some((key, i));
            }
        }
        best.map(|(_, i)| i)
    }
}

#[derive(Default)]
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
    /// Context swapped out to host memory, in tokens.
    dram_tokens: u64,
    next_step_at: Nanos,
    scheduled: bool,
    last_step_ns: Nanos,
    completed: u64,
    preemptions: u64,
    /// Records what happens to the sequences the loop asked to trace; inert otherwise.
    tracer: Tracer,
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
        self.parked.push(Parked { id: next_id, tokens, deadline, parked_at: now, swapped: false });
    }

    /// Evict one context under `policy`, choosing by `victim`. Parked context goes before a running
    /// sequence, since dropping idle context stalls nobody, and the parked entry for `keep` is
    /// exempt, because it is the one the request at the head of the queue is about to reuse. A
    /// recompute drops the context; a swap moves it to host memory and charges the copy to this
    /// step. False when there is nothing left to evict.
    fn evict(
        &mut self,
        policy: Policy,
        victim: Victim,
        keep: Option<u64>,
        cost: &CostModel,
        dram_cap: u64,
        extra_ns: &mut Nanos,
        running_too: bool,
    ) -> bool {
        let swap = |tokens: u64, dram_tokens: u64| match policy {
            Policy::Swap => true,
            Policy::SwapElseRecompute => dram_tokens + tokens <= dram_cap,
            Policy::Recompute | Policy::Never => false,
        };
        let parked = victim.pick(self.parked.iter().map(|p| {
            (!p.swapped && Some(p.id) != keep).then_some((p.parked_at, p.tokens, p.deadline))
        }));
        if let Some(i) = parked {
            let tokens = self.parked[i].tokens;
            self.kv_tokens = self.kv_tokens.saturating_sub(tokens);
            if swap(tokens, self.dram_tokens) {
                self.parked[i].swapped = true;
                self.dram_tokens += tokens;
                *extra_ns += cost.swap_ns(tokens);
            } else {
                self.parked.swap_remove(i);
            }
            self.preemptions += 1;
            return true;
        }
        if !running_too {
            return false;
        }
        let Some(i) = victim.pick(
            self.running.iter().map(|s| Some((s.admitted_at, s.resident(), s.req.deadline))),
        ) else {
            return false;
        };
        let mut s = self.running.swap_remove(i);
        let tokens = s.resident();
        self.kv_tokens = self.kv_tokens.saturating_sub(tokens);
        if swap(tokens, self.dram_tokens) {
            s.swapped = true;
            self.dram_tokens += tokens;
            *extra_ns += cost.swap_ns(tokens);
        } else {
            // Everything generated so far becomes prompt to compute again; the prefill done this
            // step, if any, is wasted work the device still did.
            s.prefill_left = tokens as u32;
        }
        self.preempted.push_front(s);
        self.preemptions += 1;
        true
    }

    /// One engine iteration at `now`. `None` when there was nothing to run, in which case the replica
    /// has parked itself and the loop schedules nothing.
    pub fn step(&mut self, sc: &Scenario, cost: &CostModel, now: Nanos) -> Option<StepOutcome> {
        let r = self;
        let policy = Policy::parse(&sc.preemption);
        let victim = Victim::parse(&sc.preemption_victim);
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
        while r.running.len() < sc.max_batch {
            // Evicted sequences first, then the queue. A queued session turn whose context is still
            // resident costs only its new tokens.
            let next_cost = if let Some(s) = r.preempted.front() {
                s.resident()
            } else if let Some(req) = r.queue.front() {
                match r.parked.iter().find(|p| p.id == req.id) {
                    Some(p) if !p.swapped => (req.prompt as u64).saturating_sub(p.tokens),
                    _ => req.prompt as u64,
                }
            } else {
                break;
            };
            if r.kv_tokens + next_cost > kv_cap {
                let keep = r.queue.front().map(|q| q.id);
                if policy != Policy::Never
                    && r.evict(policy, victim, keep, cost, dram_cap, &mut extra_ns, false)
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
                if s.swapped {
                    s.swapped = false;
                    r.dram_tokens = r.dram_tokens.saturating_sub(tokens);
                    extra_ns += cost.swap_ns(tokens);
                }
                r.kv_tokens += tokens;
                r.running.push(s);
                continue;
            }
            match r.queue.pop_front() {
                Some(req) => {
                    r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
                    let mut prefill_left = req.prompt;
                    if let Some(i) = r.parked.iter().position(|p| p.id == req.id) {
                        let p = r.parked.swap_remove(i);
                        let reused = p.tokens.min(req.prompt as u64);
                        if p.swapped {
                            r.dram_tokens = r.dram_tokens.saturating_sub(p.tokens);
                            extra_ns += cost.swap_ns(p.tokens);
                            r.kv_tokens += req.prompt as u64;
                        } else {
                            r.kv_tokens += req.prompt as u64 - reused;
                        }
                        prefill_left = req.prompt - reused as u32;
                    } else {
                        r.kv_tokens += req.prompt as u64;
                    }
                    r.running.push(Seq {
                        prefill_left,
                        output_left: req.output,
                        admitted_at: now,
                        first_token_at: 0,
                        last_token_at: 0,
                        max_itl: 0,
                        itl_sum: 0,
                        itl_count: 0,
                        swapped: false,
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
        // else's token stream.
        let mut budget = sc.step_token_budget;
        let mut prefill_tokens = 0u32;
        for s in r.running.iter_mut() {
            if budget == 0 {
                break;
            }
            if s.prefill_left > 0 {
                let take = s.prefill_left.min(budget);
                s.prefill_left -= take;
                budget -= take;
                prefill_tokens += take;
                r.tracer.prefill_chunk(s.req.id, take);
            }
        }
        let mut decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();

        // A decode step grows every decoding sequence by one token. When that would not fit, the
        // policy evicts until it does: parked context first, then running sequences by the victim
        // rule, and a sequence evicted here re-enters at the head of admission next step. A lone
        // sequence is never evicted: admission let it in over the cap, and evicting it would only
        // re-admit it next step.
        if policy != Policy::Never {
            while r.kv_tokens + decoding as u64 > kv_cap
                && r.evict(policy, victim, None, cost, dram_cap, &mut extra_ns, r.running.len() > 1)
            {
                preempted += 1;
                decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();
            }
        }

        // Step time comes from the cost model in sim-physics, the one place that formula
        // lives. Prefill and decode contend for one device, which is why a big prefill shows
        // up in everyone's inter-token latency.
        let step_ns = cost.step_ns(decoding, r.kv_tokens, prefill_tokens) + extra_ns;
        let token_at = now + step_ns;
        r.last_step_ns = step_ns;
        r.tracer.snapshot(ResourceSnapshot { start: now, end: token_at, batch_size: r.running.len() as u32, running: r.running.len() as u32, queued: r.queue.len() as u32, kv_tokens: r.kv_tokens, decoding: decoding as u32, prefill_tokens, step_ns });

        // With speculation each sequence advances by the expected tokens per step, carried as a
        // fractional credit so the long-run rate is exact; off, the credit is exactly 1.0 a step and
        // every existing run is unchanged. The tokens of one step arrive together, so the gap a user
        // sees is still the whole step (max ITL) while the mean is per token produced.
        let tokens_per_step = cost.spec_tokens_per_step();
        let mut finished: Vec<usize> = Vec::new();
        let mut generated = 0u64;
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
            if s.swapped {
                r.dram_tokens = r.dram_tokens.saturating_sub(s.resident());
            }
            victim = Some((s.req, true));
        }
        if let Some(pos) = r.parked.iter().position(|p| p.id == id) {
            let p = r.parked.swap_remove(pos);
            if p.swapped {
                r.dram_tokens = r.dram_tokens.saturating_sub(p.tokens);
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
    pub fn completed(&self) -> u64 {
        self.completed
    }
    /// The loop's handle on what this replica records about traced sequences.
    pub fn tracer_mut(&mut self) -> &mut Tracer {
        &mut self.tracer
    }
}
