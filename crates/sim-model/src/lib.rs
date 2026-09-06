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
}

#[derive(Default)]
pub struct Replica {
    queue: VecDeque<Request>,
    running: Vec<Seq>,
    queued_tokens: u64,
    /// Resident key-value tokens: for every running sequence, its prompt plus what it has generated.
    /// This is the real capacity constraint, and it is denominated in tokens rather than requests.
    kv_tokens: u64,
    next_step_at: Nanos,
    scheduled: bool,
    last_step_ns: Nanos,
    completed: u64,
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

    /// One engine iteration at `now`. `None` when there was nothing to run, in which case the replica
    /// has parked itself and the loop schedules nothing.
    pub fn step(&mut self, sc: &Scenario, cost: &CostModel, now: Nanos) -> Option<StepOutcome> {
        let r = self;
        // Admit while there is room. The replica is itself a scheduler, so this is the second
        // scheduling layer and it can disagree with the router.
        //
        // Two limits, and which one binds is the point: a sequence-count cap, and the
        // key-value token budget. A 24,000-token prompt consumes what eight chat turns
        // consume, so a queue of long requests blocks admission that a request count would
        // have allowed. Nothing here preempts; a blocked request waits, which is what
        // produces the queueing this scenario is about.
        let kv_cap = sc.kv_capacity_tokens as u64;
        while r.running.len() < sc.max_batch {
            let next_cost = match r.queue.front() {
                Some(req) => req.prompt as u64,
                None => break,
            };
            if r.kv_tokens + next_cost > kv_cap && !r.running.is_empty() {
                break;
            }
            match r.queue.pop_front() {
                Some(req) => {
                    r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
                    r.kv_tokens += req.prompt as u64;
                    r.running.push(Seq {
                        prefill_left: req.prompt,
                        output_left: req.output,
                        admitted_at: now,
                        first_token_at: 0,
                        last_token_at: 0,
                        max_itl: 0,
                        itl_sum: 0,
                        itl_count: 0,
                        req,
                    });
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
            }
        }
        let decoding = r.running.iter().filter(|s| s.prefill_left == 0).count();

        // Step time comes from the cost model in sim-physics, the one place that formula
        // lives. Prefill and decode contend for one device, which is why a big prefill shows
        // up in everyone's inter-token latency.
        let step_ns = cost.step_ns(decoding, r.kv_tokens, prefill_tokens);
        let token_at = now + step_ns;
        r.last_step_ns = step_ns;

        let mut finished: Vec<usize> = Vec::new();
        for (idx, s) in r.running.iter_mut().enumerate() {
            if s.prefill_left > 0 {
                continue;
            }
            if s.first_token_at == 0 {
                s.first_token_at = token_at;
            } else {
                let gap = token_at - s.last_token_at;
                s.max_itl = s.max_itl.max(gap);
                s.itl_sum += gap;
                s.itl_count += 1;
            }
            s.last_token_at = token_at;
            s.output_left = s.output_left.saturating_sub(1);
            if s.output_left == 0 {
                finished.push(idx);
            }
        }
        r.kv_tokens += decoding as u64;

        let mut retired: Vec<FinishedSeq> = Vec::with_capacity(finished.len());
        for idx in finished.iter().rev() {
            let s = r.running.swap_remove(*idx);
            r.kv_tokens = r
                .kv_tokens
                .saturating_sub(s.req.prompt as u64 + s.req.output as u64);
            r.completed += 1;
            let mean_itl = if s.itl_count > 0 { s.itl_sum / s.itl_count as Nanos } else { 0 };
            retired.push(FinishedSeq {
                req: s.req,
                admitted_at: s.admitted_at,
                first_token_at: s.first_token_at,
                max_itl: s.max_itl,
                mean_itl,
            });
        }

        let idle = r.running.is_empty() && r.queue.is_empty();
        if idle {
            r.scheduled = false;
        } else {
            r.next_step_at = token_at;
        }
        Some(StepOutcome { step_ns, token_at, finished: retired, idle })
    }

    /// Pull a request out wherever it is. The bool says whether it was running, because where it
    /// died decides how expensive the failure was.
    pub fn remove(&mut self, id: u64) -> Option<(Request, bool)> {
        let r = self;
        let mut victim: Option<(Request, bool)> = None;
        if let Some(pos) = r.queue.iter().position(|x| x.id == id) {
            let req = r.queue.remove(pos).unwrap();
            r.queued_tokens = r.queued_tokens.saturating_sub(req.prompt as u64);
            victim = Some((req, false));
        } else if let Some(pos) = r.running.iter().position(|s| s.req.id == id) {
            let s = r.running.swap_remove(pos);
            let generated = s.req.output.saturating_sub(s.output_left) as u64;
            r.kv_tokens = r.kv_tokens.saturating_sub(s.req.prompt as u64 + generated);
            victim = Some((s.req, true));
        }
        victim
    }

    /// Queued plus running: the load a balancer is trying to even out.
    pub fn load(&self) -> usize {
        self.queue.len() + self.running.len()
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
            + self.running.iter().map(|s| s.prefill_left as u64 + s.output_left as u64).sum::<u64>()
    }
    pub fn kv_tokens(&self) -> u64 {
        self.kv_tokens
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
}
