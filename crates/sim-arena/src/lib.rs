//! The mechanical half of the policy arena referee.
//!
//! `docs/arena.md` section 2.3 splits the referee in two and argues for keeping the mechanical half
//! much larger than the judging half: everything that can be a check should be a check, so the agent
//! only ever handles the residual. This module is that mechanical half, as far as today's engine can
//! carry it:
//!
//! | `docs/arena.md` says | here |
//! |---|---|
//! | SLA gate and worst-case scoring, section 1 | [`score_policy`], [`score_run`] |
//! | rated-capacity honesty, section 3 | [`measure_honest_capacity`], [`HonestCapacity`] |
//! | realism envelope on every load parameter, section 2.2 | [`check_realism`], [`envelope`] |
//! | the fixed held-out suite, section 5 | `scenarios/holdout/*.txt`, [`holdout_suite`] |
//! | a round: same seeds for every policy, section 4 | [`run_round`] |
//! | determinism: identical fingerprint on replay | [`replay_is_deterministic`] |
//! | reproducibility: config plus seed recorded | [`Cell::seed`], [`Cell::fingerprint`] |
//!
//! Two rows of that table are **not** here and cannot be until earlier milestones land, and they are
//! named rather than quietly skipped:
//!
//! - **Physics invariants in strict mode.** There is no referee in the engine yet, so "a violation
//!   aborts the run and the candidate scores nothing" has nothing to hook into. The scoring here is
//!   written so that a run which fails a check contributes a hard zero rather than being dropped,
//!   which is the shape the strict referee will need.
//! - **A policy declaring its own rated capacity.** `PolicySpec` has no such field yet
//!   (`docs/arena.md` section 6.2 flags it as a proto change), so [`ScoreConfig::declared_rated_rps`]
//!   lets the caller supply the declaration and defaults to the scenario's *analytic* capacity from
//!   `Scenario::rated_rps`. Until a policy declares, honesty cannot be violated, only measured, which
//!   is what [`measure_honest_capacity`] is for.
//!
//! Nothing in here is parallel or wall-clock dependent: a round is a nested loop in a fixed order
//! with the seed taken from the scenario file, so every policy meets byte-identical load.

use sim_metrics::Outcome;
use sim_scenario::Scenario;
use sim_leaf::{self as sim, RunResult};
use sim_workload::Workload;

/// The cap from `docs/arena.md` section 1: *"keeping out-of-SLO sessions under an SLA cap (say
/// 99.9%)"*.
///
/// It is the default and not a constant, because today's engine does not reach it at any offered
/// rate on the held-out suite (see `docs/arena-implementation.md`), and a gate that every candidate
/// fails ranks nothing. Run with a looser cap while the engine is being calibrated, and record which
/// cap a score was earned under — the same discipline section 2.3 demands for rule sets.
pub const DEFAULT_SLA_CAP: f64 = 0.999;

/// Warm-up floor from `docs/arena.md` section 3: *"warm-up longer than a cold start, so at least
/// several minutes of simulated time"*, because a policy declaring capacity from a transient is
/// declaring nothing.
///
/// Unenforceable today: there is no autoscaling and no cold start in the engine, so there is nothing
/// for several minutes of warm-up to settle. [`RoundConfig::min_warmup_s`] therefore defaults to the
/// much smaller floor below, and this constant is what it becomes once autoscaling lands.
pub const ARENA_MIN_WARMUP_S: f64 = 180.0;

/// What the round runner actually enforces today: enough warm-up for an empty fleet to fill and for
/// one telemetry round trip to have happened several times over.
pub const WARMUP_FLOOR_TODAY_S: f64 = 10.0;

// ---------------------------------------------------------------------------------------------
// 1. Offered load
// ---------------------------------------------------------------------------------------------

/// The offered load of a scenario over its *measured* window, which is the only window the referee
/// scores (`docs/ARCHITECTURE.md` section 8 excludes warm-up, and `docs/arena.md` section 3 requires
/// that the declaration happen before it).
///
/// Both numbers are needed and they answer different questions. The mean decides whether a load is
/// in scope for the worst-case minimum at all; the peak plus [`free_shed_fraction`] decides how much
/// of the traffic a policy was allowed to spill for free.
///
/// [`free_shed_fraction`]: OfferedLoad::free_shed_fraction
#[derive(Clone, Copy, Debug)]
pub struct OfferedLoad {
    pub mean_rps: f64,
    pub peak_rps: f64,
    /// Time-average of `max(0, rate(t) - rated)` over the measured window, in requests per second.
    /// Kept separately from the mean because a shape can offer a modest mean and still spend thirty
    /// seconds far above any rated capacity, and those are different loads.
    excess_integral_rps: f64,
}

impl OfferedLoad {
    /// Fraction of offered requests a policy may shed without SLO cost, per `docs/arena.md`
    /// section 3: *"The policy is allowed to spill traffic above its rated capacity without SLO
    /// cost."*
    ///
    /// Mechanically that is the share of the offered rate that sat above the declared capacity,
    /// integrated over the measured window — the capacity being the one [`offered_load`] was called
    /// with, so the exemption and the scope test cannot be computed against different numbers. A load entirely below capacity yields zero, a load
    /// permanently at twice capacity yields a half, and a load that bursts to 3x for a quarter of the
    /// window yields the exact area — one rule covering all three.
    pub fn free_shed_fraction(&self) -> f64 {
        if self.mean_rps <= 0.0 {
            return 0.0;
        }
        (self.excess_integral_rps / self.mean_rps).clamp(0.0, 1.0)
    }
}

/// Offered load of a scenario, sampled from the same `Workload::rate_at` the simulator drives
/// arrivals with, so the referee and the engine cannot disagree about what was offered.
pub fn offered_load(sc: &Scenario, rated_rps: f64) -> OfferedLoad {
    let from = sc.warmup_s.max(0.0);
    let to = sc.duration_s.max(from);
    // A fixed grid rather than the piecewise breakpoints: the step change is the only shape today,
    // but a grid stays correct when the load generator gains diurnal and burst shapes, and 4,096
    // points over a two-minute run is finer than the sample interval.
    const GRID: usize = 4096;
    let mut sum = 0.0;
    let mut excess = 0.0;
    let mut peak = 0.0f64;
    for i in 0..GRID {
        let t = from + (to - from) * (i as f64 + 0.5) / GRID as f64;
        let r = Workload::rate_at(sc, t);
        sum += r;
        excess += (r - rated_rps).max(0.0);
        peak = peak.max(r);
    }
    OfferedLoad {
        mean_rps: sum / GRID as f64,
        peak_rps: peak,
        excess_integral_rps: excess / GRID as f64,
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Scoring, docs/arena.md section 1
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ScoreConfig {
    /// The gate. Breaching it scores zero rather than costing points, per section 1.
    pub sla_cap: f64,
    /// What the policy declared, if anything. `None` falls back to the scenario's analytic
    /// `rated_rps()`, which is the engine's own estimate rather than a commitment by the policy — so
    /// with `None` there is no honesty to test, only capacity to measure.
    pub declared_rated_rps: Option<f64>,
    /// Set false to reproduce the engine's own `slo_attainment()` denominator. Default true, and the
    /// difference matters: see [`arena_attainment`].
    pub count_failures_as_out_of_slo: bool,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        ScoreConfig {
            sla_cap: DEFAULT_SLA_CAP,
            declared_rated_rps: None,
            count_failures_as_out_of_slo: true,
        }
    }
}

impl ScoreConfig {
    pub fn with_cap(sla_cap: f64) -> Self {
        ScoreConfig { sla_cap, ..Default::default() }
    }
}

/// SLO attainment as the *arena* must define it, which is not what `RunResult::slo_attainment`
/// returns.
///
/// The engine's version divides by successful requests only, so a rejection or a client timeout
/// leaves the denominator rather than failing it. Under that definition a policy that sheds nine
/// requests in ten reports perfect attainment, which is precisely the trade `docs/arena.md`
/// section 1 exists to forbid. The arena therefore counts **every** request that arrived in the
/// measured window: `Rejected`, `TimeoutQueued` and `TimeoutRunning` are all out of SLO.
///
/// The one exemption is section 3's, and it is why `free_shed` is a parameter: traffic offered above
/// the declared rated capacity may be spilled for free, so that share is removed from the
/// denominator. Pass `0.0` for the unexempted number.
pub fn arena_attainment(r: &RunResult, free_shed: f64) -> f64 {
    let total = r.records.len();
    if total == 0 {
        return f64::NAN;
    }
    let ok = r.records.iter().filter(|x| x.outcome == Outcome::Ok).count();
    let denom = total as f64 * (1.0 - free_shed.clamp(0.0, 1.0));
    if denom <= 0.0 {
        return 1.0;
    }
    (ok as f64 / denom).min(1.0)
}

/// One (policy, load) pair, scored.
#[derive(Clone, Debug)]
pub struct RunScore {
    pub load: String,
    pub policy: String,
    pub seed: u64,
    pub fingerprint: u64,
    pub offered: OfferedLoad,
    pub rated_rps: f64,
    /// True when `offered.mean_rps <= rated_rps`, i.e. when this load counts toward the worst case at
    /// all. Section 1's `where` clause.
    pub in_scope: bool,
    pub free_shed_fraction: f64,
    /// Fraction of *all* measured requests that finished inside the SLO, after the free-shedding
    /// exemption. This is what the gate reads.
    pub attainment: f64,
    /// The engine's own figure, over successful requests only. Kept beside the arena figure because
    /// the gap between them is the size of the shedding loophole on that run.
    pub engine_attainment: f64,
    pub goodput_tokens_s: f64,
    /// Goodput as a fraction of the output tokens the load offered. A diagnostic, not the score.
    ///
    /// Section 1 scores the *minimum over loads of goodput*, and goodput is an absolute rate, so the
    /// minimum across a slate whose loads differ by 20x in offered work is decided by the smallest
    /// load rather than by the hardest one — which is visible in the measured round in
    /// `docs/arena-implementation.md`. This is the same quantity made comparable across loads, and it
    /// is what a normalized objective would use. Reported so the distortion is measurable rather than
    /// argued about.
    pub goodput_share: f64,
    pub throughput_tokens_s: f64,
    pub completed_rps: f64,
    pub load_imbalance_cv: f64,
    /// Zero when the gate is breached, goodput otherwise. Meaningless unless `in_scope`.
    pub score: f64,
    /// The cap this run was scored under. Recorded per run because section 2.3 requires that a score
    /// remember which rule set earned it.
    pub gate_cap: f64,
    /// Set when a mechanical check failed. A failed check is a hard zero, not an omission, so the
    /// strict physics referee can hook in here unchanged.
    pub violations: Vec<String>,
}

impl RunScore {
    /// True when the SLA gate was breached. `!(x >= cap)` rather than `x < cap` so a NaN attainment,
    /// which means nothing was measured, counts as a breach instead of passing.
    pub fn gated(&self) -> bool {
        !(self.attainment >= self.gate_cap)
    }
}

/// Score one completed run.
pub fn score_run(r: &RunResult, cfg: &ScoreConfig) -> RunScore {
    let rated = cfg.declared_rated_rps.unwrap_or(r.rated_rps);
    let offered = offered_load(&r.scenario, rated);
    let free_shed = offered.free_shed_fraction();
    let attainment = if cfg.count_failures_as_out_of_slo {
        arena_attainment(r, free_shed)
    } else {
        r.slo_attainment()
    };
    let in_scope = offered.mean_rps <= rated;
    let goodput = r.goodput_tokens_s();
    // Offered output tokens per second from the mixture's own means. Retries are excluded, so a
    // scenario with retries can show a share above what its first attempts alone would explain.
    let sc = &r.scenario;
    let o_mean = sc.output_mean * (1.0 - sc.long_probability)
        + sc.long_output_mean * sc.long_probability;
    let offered_tokens_s = offered.mean_rps * o_mean;
    // NaN attainment means no requests were measured at all, which is a broken load rather than a
    // perfect policy, so it gates.
    let passes = attainment >= cfg.sla_cap;
    RunScore {
        load: r.scenario.name.clone(),
        policy: r.routing_label.clone(),
        seed: r.scenario.seed,
        fingerprint: r.fingerprint,
        offered,
        rated_rps: rated,
        in_scope,
        free_shed_fraction: free_shed,
        attainment,
        engine_attainment: r.slo_attainment(),
        goodput_tokens_s: goodput,
        goodput_share: if offered_tokens_s > 0.0 { goodput / offered_tokens_s } else { f64::NAN },
        throughput_tokens_s: r.throughput_tokens_s(),
        completed_rps: r.completed_rps(),
        load_imbalance_cv: r.load_imbalance_cv(),
        score: if passes { goodput } else { 0.0 },
        gate_cap: cfg.sla_cap,
        violations: Vec::new(),
    }
}

/// A policy's score over a slate of loads: the minimum, per section 1.
#[derive(Clone, Debug)]
pub struct PolicyScore {
    pub policy: String,
    /// `min` over in-scope loads of the gated goodput. `None` when no load was in scope, which is
    /// itself a finding rather than a zero: the policy was never tested inside its own claim.
    pub score: Option<f64>,
    /// Which load produced the minimum. The interesting half of a worst-case score.
    pub worst_load: Option<String>,
    /// Mean gated goodput over in-scope loads, reported only so the gap to `score` is visible. It is
    /// deliberately not the score: section 1 is a minimum *because* the guarantee must hold against
    /// an adversarial load generator.
    pub mean_score: Option<f64>,
    /// Mean *ungated* goodput over in-scope loads. Not a score under any reading of section 1, and
    /// present for exactly one reason: when every candidate is gated to zero — which is the state of
    /// today's engine against a 99.9% cap — a ranking keyed only on the score falls back to
    /// alphabetical order, which reads as a result and is not one. This breaks that tie by measured
    /// capability instead, and the report prints it so the tie is visible rather than implied.
    pub mean_goodput: Option<f64>,
    pub loads_in_scope: usize,
    pub loads_out_of_scope: usize,
    pub gate_breaches: usize,
    pub runs: Vec<RunScore>,
}

/// Worst-case score over a slate of runs of one policy.
pub fn score_policy(policy: &str, runs: Vec<RunScore>, _cfg: &ScoreConfig) -> PolicyScore {
    let mut worst: Option<(f64, String)> = None;
    let mut sum = 0.0;
    let mut sum_goodput = 0.0;
    let mut n = 0usize;
    let mut out = 0usize;
    let mut breaches = 0usize;
    for r in &runs {
        if r.gated() {
            breaches += 1;
        }
        if !r.in_scope {
            out += 1;
            continue;
        }
        n += 1;
        sum += r.score;
        sum_goodput += r.goodput_tokens_s;
        if worst.as_ref().map(|(s, _)| r.score < *s).unwrap_or(true) {
            worst = Some((r.score, r.load.clone()));
        }
    }
    PolicyScore {
        policy: policy.to_string(),
        score: worst.as_ref().map(|(s, _)| *s),
        worst_load: worst.map(|(_, l)| l),
        mean_score: if n > 0 { Some(sum / n as f64) } else { None },
        mean_goodput: if n > 0 { Some(sum_goodput / n as f64) } else { None },
        loads_in_scope: n,
        loads_out_of_scope: out,
        gate_breaches: breaches,
        runs,
    }
}

// ---------------------------------------------------------------------------------------------
// 3. Rated-capacity honesty, docs/arena.md section 3
// ---------------------------------------------------------------------------------------------

/// What a policy's *honest* rated capacity would have been, measured rather than declared.
///
/// Section 3 makes the declaration a two-sided commitment: too low and the policy sheds traffic it
/// could have served, too high and it must meet the gate on load it cannot handle. No policy declares
/// anything yet, so the arena cannot punish a lie — but the number the declaration is supposed to
/// approximate is measurable today, and measuring it now is what makes the mechanism testable the
/// moment the field exists.
#[derive(Clone, Debug)]
pub struct HonestCapacity {
    pub policy: String,
    pub sla_cap: f64,
    /// Highest offered rate at which the policy still met the cap. Section 3's number.
    pub highest_passing_rps: Option<f64>,
    /// Highest rate below which the policy met the cap at *every* sampled rate. A declaration is a
    /// promise about all load up to a number, so this is the defensible one to declare; where it is
    /// below `highest_passing_rps` the attainment curve is not monotone and the gap is worth looking
    /// at rather than smoothing over.
    pub monotone_rps: Option<f64>,
    /// The analytic figure from `Scenario::rated_rps()`, for comparison. The cost model's claim about
    /// the fleet, which the arena is in a position to falsify.
    pub analytic_rps: f64,
    /// `(offered mean rps, arena attainment, goodput tokens/s)` at each sampled rate, in rate order.
    pub curve: Vec<(f64, f64, f64)>,
}

/// Run `base` at each rate with `policy` and find the honest rated capacity.
///
/// Every run keeps the scenario's own seed, so the rates differ and nothing else does.
pub fn measure_honest_capacity(
    base: &Scenario,
    policy: &str,
    rates: &[f64],
    sla_cap: f64,
) -> Result<HonestCapacity, String> {
    let mut curve = Vec::new();
    let mut sorted: Vec<f64> = rates.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for rate in &sorted {
        let mut sc = base.clone();
        sc.routing = policy.to_string();
        sc.arrival_rps = *rate;
        sc.name = format!("{} @ {:.0} rps", base.name, rate);
        let run = sim::run(&sc)?;
        // No free shedding here: the question is what the policy can actually serve, so nothing is
        // exempted. Exempting above-capacity spill while measuring capacity would be circular.
        let att = arena_attainment(&run, 0.0);
        curve.push((offered_load(&sc, f64::INFINITY).mean_rps, att, run.goodput_tokens_s()));
    }
    let highest = curve
        .iter()
        .filter(|(_, a, _)| *a >= sla_cap)
        .map(|(r, _, _)| *r)
        .fold(None::<f64>, |acc, r| Some(acc.map_or(r, |a: f64| a.max(r))));
    let mut monotone = None;
    for (r, a, _) in &curve {
        if *a >= sla_cap {
            monotone = Some(*r);
        } else {
            break;
        }
    }
    Ok(HonestCapacity {
        policy: policy.to_string(),
        sla_cap,
        highest_passing_rps: highest,
        monotone_rps: monotone,
        analytic_rps: base.rated_rps(),
        curve,
    })
}

// ---------------------------------------------------------------------------------------------
// 4. The realism envelope, docs/arena.md section 2.2
// ---------------------------------------------------------------------------------------------

/// One parameter's measured range.
///
/// `source` names the section of `docs/calibration.md` the bound came from, verbatim enough to check.
/// Where that document says a quantity is unmeasured, the bound is deliberately wide and the source
/// string says so — a wide bound that admits it is a bound; a narrow bound invented to look rigorous
/// would reject realistic load, which is the one thing section 2.2 must not do.
#[derive(Clone, Copy, Debug)]
pub struct Bound {
    pub key: &'static str,
    pub lo: f64,
    pub hi: f64,
    pub source: &'static str,
}

#[derive(Clone, Debug)]
pub struct Violation {
    pub key: String,
    pub value: f64,
    pub lo: f64,
    pub hi: f64,
    pub source: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} = {} outside [{}, {}] ({})",
            self.key, self.value, self.lo, self.hi, self.source
        )
    }
}

/// The envelope. Every workload parameter the scenario format exposes, with the measured range it has
/// to sit inside and where that range came from.
///
/// Section 2.2's standard: *"A load of a million-token prompts at ten thousand requests per second is
/// not a clever attack, it is an invalid submission, and the referee should reject it without needing
/// judgement."*
pub fn envelope() -> Vec<Bound> {
    vec![
        // -- arrivals ------------------------------------------------------------------------
        Bound {
            key: "arrival_rps",
            lo: 0.1,
            hi: 3_000.0,
            // calibration.md section 10: measured fleet mean rates are 45.15 rps (Azure 2024 conv),
            // 27.78 (Azure 2024 code) and 6.67 (Mooncake, 1 h). Section 2.3 measures diurnal
            // peak/trough at 8.96x (Azure code, hour-of-day averaged), 33.1x worst single hour, and
            // 34.8x (BurstGPT_3 conversation). 45 x 34.8 = 1,566, doubled for a fleet larger than any
            // of the three traces, since section 9.2 records that no public source gives a fleet size.
            source: "calibration.md 10 (mean rates 6.67-45.15) x 2.3 (diurnal 34.8x worst)",
        },
        Bound {
            key: "load_step_factor",
            lo: 0.028,
            hi: 34.8,
            // Section 2.3, measured peak/trough by hour-of-day: 34.8x for BurstGPT_3 conversation,
            // 33.1x between the single busiest and quietest hour of Azure 2024 code. The reciprocal
            // is the same swing downward, which is a real shape: a trough is a load too.
            source: "calibration.md 2.3 (peak/trough 34.8x measured; 1/34.8 for the trough)",
        },
        Bound {
            key: "load_step_duration_s",
            lo: 1.0,
            hi: 86_400.0,
            // UNMEASURED, and calibration.md says so twice: section 2.4 "No source we found reports a
            // burst-duration distribution for LLM serving traffic", offering only 10-60 s inferred
            // from IDC saturation; section 9.1 gives burst duration a [GUESS] default of 30 s with a
            // sweep range of 5-300 s. A step can also be diurnal rather than a burst, which is hours.
            // So the bound is a day, wide on purpose.
            source: "calibration.md 2.4/9.1 UNMEASURED [GUESS] 30 s, sweep 5-300 s; wide bound to a day",
        },
        // -- lengths, calibration.md section 3.1 ---------------------------------------------
        Bound {
            key: "prompt_mean",
            lo: 60.0,
            hi: 130_000.0,
            // Section 3.1 spans LMSYS-Chat-1M input mean 69.5 at the low end to TraceLab coding-agent
            // ~127k (126k prefix + 857 append, medians) at the high end, with Azure conv 1,632,
            // Azure code 2,511, Mooncake toolagent 8,596 and Mooncake conversation 12,035 between.
            source: "calibration.md 3.1 (LMSYS 69.5 to TraceLab ~127k)",
        },
        Bound {
            key: "long_prompt_mean",
            lo: 60.0,
            hi: 1_000_000.0,
            // Same table's tail rather than its means: Mooncake conversation input p99 = 85,401 and
            // TraceLab reports a 918k-token prefix p99. One million is that p99 rounded up, and it is
            // the point at which section 2.2's "million-token prompts" example becomes invalid.
            source: "calibration.md 3.1 (Mooncake p99 85,401; TraceLab prefix p99 918k)",
        },
        Bound {
            key: "prompt_cv",
            lo: 0.5,
            hi: 3.0,
            // Section 3.3 measures input CV at 0.850 (Azure 2024 code), 0.938 (Azure conv), 0.960-0.964
            // (Azure 2023) and 2.785 (BurstGPT_3 all); section 10 records 1.28 for Mooncake toolagent.
            // Rounded outward from 0.85-2.79.
            source: "calibration.md 3.3 (input CV 0.85-2.79 measured)",
        },
        Bound {
            key: "output_mean",
            lo: 8.0,
            hi: 1_500.0,
            // Low end: Azure 2024 code output mean 22.7 with p50 = 8 (section 3.1), and 8 is the floor
            // an inline-completion service can plausibly sit at. High end: the largest measured mean is
            // Mooncake conversation at 343; section 9.1 records that reasoning models add reasoning
            // tokens at "on average 4x longer than answer lengths" with a 1x-10x sweep range, and that
            // multiplier is itself a [GUESS], so 343 x 4 rounded up.
            source: "calibration.md 3.1 (means 22.7-343) x 9.1 reasoning 4x [GUESS]",
        },
        Bound {
            key: "long_output_mean",
            lo: 8.0,
            hi: 7_000.0,
            // Section 3.1 output p99s: 694 (Azure conv), 898 (Mooncake toolagent), 1,067 (BurstGPT
            // conv), 6,571 (TraceLab Claude Code). Rounded up from the largest measured p99.
            source: "calibration.md 3.1 (output p99 up to 6,571, TraceLab)",
        },
        Bound {
            key: "output_cv",
            lo: 0.7,
            hi: 3.4,
            // Section 3.4's table is exactly this range, measured end to end: 0.73 (Mooncake
            // conversation) to 3.30 (Azure 2024 code). The section's own summary line reads
            // "Range: 0.7 to 3.3".
            source: "calibration.md 3.4 (output CV 0.73-3.30 measured, stated as 0.7-3.3)",
        },
        Bound {
            key: "long_probability",
            lo: 0.0,
            hi: 0.76,
            // WEAKLY SOURCED. The engine's two-mode mixture is not a quantity calibration.md measures;
            // section 3.3 establishes that input length is multi-modal ("consistent with a mixture of
            // distinct application prompt templates") without giving mode weights. The nearest measured
            // proxy is section 4.2's continuation fraction, whose endpoints are measured (3.3% fleet-wide
            // to 76.1% inside chat-UI traffic) with a [GUESS] fleet default of 0.25 in section 9.1.
            source: "calibration.md 4.2 continuation fraction 0.033-0.761 as the nearest measured proxy; mode weight itself UNMEASURED",
        },
        // -- client behaviour, calibration.md section 8 --------------------------------------
        Bound {
            key: "client_timeout_s",
            lo: 1.0,
            hi: 600.0,
            // Section 8.1, read from SDK source: DEFAULT_TIMEOUT is 600 s in both openai-python and
            // anthropic-sdk-python, connect timeout 5 s. Note the section's other finding, which this
            // bound cannot express: three of the most widely deployed configurations have *no* timeout
            // at all, and "a simulator should model 'client waits forever' as a real state". The
            // scenario format has no way to say infinity, so that regime is outside the envelope
            // because it is outside the engine.
            source: "calibration.md 8.1 (SDK DEFAULT_TIMEOUT 600 s, connect 5 s)",
        },
        Bound {
            key: "max_attempts",
            lo: 1.0,
            hi: 18.0,
            // Section 8.1: DEFAULT_MAX_RETRIES = 2, so 3 attempts by default; several clients default
            // to 0 retries, so 1 attempt is the floor. Section 8.2 documents the layering that sets
            // the ceiling: a tenacity stop_after_attempt(6) wrapper over the SDK's 2 is "up to 18
            // requests per logical call", which Azure's own quota documentation warns about.
            source: "calibration.md 8.1/8.2 (SDK 3 attempts; documented layering up to 18)",
        },
        Bound {
            key: "retry_backoff_s",
            lo: 0.375,
            hi: 8.0,
            // Section 8.2, from openai-python _calculate_retry_timeout:
            // sleep = min(0.5 * 2^attempt, 8.0) * (1 - 0.25 * random()). So the smallest possible sleep
            // is 0.5 x 0.75 = 0.375 s and the largest is the 8.0 s cap.
            source: "calibration.md 8.2 (min(0.5*2^n, 8.0)*(1-0.25*rand) => 0.375-8.0 s)",
        },
        Bound {
            key: "retry_budget_fraction",
            lo: 0.0,
            hi: 1.0,
            // Section 9.1: retry-driven share of offered load is not published. [GUESS] default 3% at
            // steady state, "up to 40% during an incident", sweep range 0-100%. The bound is the sweep
            // range, which is the whole interval.
            source: "calibration.md 9.1 UNMEASURED [GUESS] 3%, incident 40%, sweep 0-100%",
        },
        // -- telemetry, calibration.md section 9.2 -------------------------------------------
        Bound {
            key: "telemetry_interval_ms",
            lo: 100.0,
            hi: 30_000.0,
            // Section 9.2: router metric staleness in production is not published. [GUESS] 5 s scrape
            // interval, sweep 1-30 s, with the primer quoted as 1-15 s. The lower bound is below
            // anything measured and is there because the engine allows it and the herding study needs
            // the fast end.
            source: "calibration.md 9.2 UNMEASURED [GUESS] 5 s, sweep 1-30 s",
        },
        Bound {
            key: "telemetry_delay_ms",
            lo: 0.0,
            hi: 30_000.0,
            // UNMEASURED anywhere in calibration.md: it records scrape *interval* as a guess and says
            // nothing about delivery delay. Bounded by the same 30 s sweep ceiling for want of anything
            // better, and flagged.
            source: "calibration.md 9.2 UNMEASURED (no delivery-delay figure exists); bounded by the 30 s staleness sweep",
        },
        // -- fleet shape --------------------------------------------------------------------
        Bound {
            key: "replicas",
            lo: 1.0,
            hi: 2_000.0,
            // Section 9.2 is explicit: "Not one public source gives a fleet size, a replica count, an
            // SLO target, a queue-depth distribution, or a KV-utilization distribution." The closest
            // published figure is DeepSeek's 226.75 average H800 nodes over one 24 h snapshot, so the
            // bound is an order of magnitude above that and is a guard against nonsense rather than a
            // calibrated range.
            source: "calibration.md 9.2 UNMEASURED (DeepSeek 226.75 nodes is the only published figure)",
        },
        Bound {
            key: "max_queue",
            lo: 1.0,
            hi: 100_000.0,
            // Section 9.2, admission control at overload: [GUESS] "drop at queue depth > 4x service
            // capacity", no range given. Wide bound, flagged.
            source: "calibration.md 9.2 UNMEASURED [GUESS] 4x service capacity",
        },
        // -- SLO definition -----------------------------------------------------------------
        // These are not workload parameters, but they decide what goodput *means*, so a load
        // generator that could move them could win by redefining the target.
        Bound {
            key: "ttft_slo_ms",
            lo: 100.0,
            hi: 600_000.0,
            source: "calibration.md 9.1 SLO class mix: 'Nothing public at all. Not one trace carries an SLO or priority label.' Ceiling is the 600 s SDK timeout, 8.1",
        },
        Bound {
            key: "itl_slo_ms",
            lo: 10.0,
            hi: 1_000.0,
            // No published ITL SLO either. The floor is below the 10.25 ms measured batch-1 decode for
            // a 70B on 8xH100 (section 6.5), so an SLO under it would be unmeetable by construction;
            // the ceiling is a second per token, past which nothing is interactive.
            source: "calibration.md 9.1 UNMEASURED; floor from 6.5 (batch-1 decode 10.25 ms)",
        },
        Bound {
            key: "e2e_slo_s",
            lo: 1.0,
            hi: 600.0,
            source: "calibration.md 9.1 UNMEASURED; ceiling is the 600 s SDK timeout, 8.1",
        },
    ]
}

/// Check a proposed scenario against the envelope. Returns every violation with the offending value,
/// because a load generator that gets told only "invalid" learns nothing and will resubmit.
pub fn check_realism(sc: &Scenario) -> Vec<Violation> {
    let mut v = Vec::new();
    let step_duration = if sc.load_step_at_s < 0.0 {
        f64::NAN
    } else {
        let until = if sc.load_step_until_s < 0.0 { sc.duration_s } else { sc.load_step_until_s };
        until - sc.load_step_at_s
    };
    for b in envelope() {
        let value = match b.key {
            "arrival_rps" => sc.arrival_rps,
            "load_step_factor" => {
                if sc.load_step_at_s < 0.0 {
                    continue;
                }
                sc.load_step_factor
            }
            "load_step_duration_s" => {
                if step_duration.is_nan() {
                    continue;
                }
                step_duration
            }
            "prompt_mean" => sc.prompt_mean,
            "long_prompt_mean" => sc.long_prompt_mean,
            "prompt_cv" => sc.prompt_cv,
            "output_mean" => sc.output_mean,
            "long_output_mean" => sc.long_output_mean,
            "output_cv" => sc.output_cv,
            "long_probability" => sc.long_probability,
            "client_timeout_s" => sc.client_timeout_s,
            "max_attempts" => sc.max_attempts as f64,
            "retry_backoff_s" => sc.retry_backoff_s,
            "retry_budget_fraction" => sc.retry_budget_fraction,
            "telemetry_interval_ms" => sc.telemetry_interval_ms,
            "telemetry_delay_ms" => sc.telemetry_delay_ms,
            "replicas" => sc.replicas as f64,
            "max_queue" => sc.max_queue as f64,
            "ttft_slo_ms" => sc.ttft_slo_ms,
            "itl_slo_ms" => sc.itl_slo_ms,
            "e2e_slo_s" => sc.e2e_slo_s,
            _ => continue,
        };
        if !(value >= b.lo && value <= b.hi) {
            v.push(Violation {
                key: b.key.to_string(),
                value,
                lo: b.lo,
                hi: b.hi,
                source: b.source.to_string(),
            });
        }
    }

    // Relational checks. Per-parameter bounds cannot catch these, and the second one is the
    // highest-leverage workload parameter in calibration.md.
    if sc.long_probability > 0.0 && sc.long_prompt_mean < sc.prompt_mean {
        v.push(Violation {
            key: "long_prompt_mean/prompt_mean".into(),
            value: sc.long_prompt_mean / sc.prompt_mean,
            lo: 1.0,
            hi: f64::INFINITY,
            source: "structural: the long mode of the mixture must be the longer one, or the label is a lie".into(),
        });
    }
    let p_mean = sc.prompt_mean * (1.0 - sc.long_probability) + sc.long_prompt_mean * sc.long_probability;
    let o_mean = sc.output_mean * (1.0 - sc.long_probability) + sc.long_output_mean * sc.long_probability;
    if o_mean > 0.0 {
        let ratio = p_mean / o_mean;
        // calibration.md 3.2 orders the measured input:output ratios from LMSYS arena chat at 0.32:1
        // to Azure 2024 code at 111:1, with the Mooncake L-Eval long-context QA benchmark in section
        // 3.1 at 264:1 as the extreme. "It is the highest-leverage workload parameter in the whole
        // simulator and it must be a swept axis, never a constant" — so it is checked, not fixed.
        if !(0.32..=264.0).contains(&ratio) {
            v.push(Violation {
                key: "input_output_ratio_of_means".into(),
                value: ratio,
                lo: 0.32,
                hi: 264.0,
                source: "calibration.md 3.2 (LMSYS 0.32:1 to Azure code 111:1) and 3.1 (L-Eval 264:1)".into(),
            });
        }
    }
    v
}

/// The warm-up check from `docs/arena.md` section 3, kept separate from the realism envelope because
/// it is a rule about the *experiment* rather than about the load.
pub fn check_warmup(sc: &Scenario, min_warmup_s: f64) -> Option<Violation> {
    if sc.warmup_s >= min_warmup_s {
        return None;
    }
    Some(Violation {
        key: "warmup_s".into(),
        value: sc.warmup_s,
        lo: min_warmup_s,
        hi: sc.duration_s,
        source: "arena.md 3: capacity is declared during warm-up, so warm-up must be longer than a cold start".into(),
    })
}

// ---------------------------------------------------------------------------------------------
// 5. The fixed held-out suite, docs/arena.md section 5
// ---------------------------------------------------------------------------------------------

/// The frozen suite, in stable order. `docs/arena.md` section 5: *"Never added to, never tuned, and
/// no generator may propose changes to it."*
///
/// The filenames carry the order, so a listing and this list cannot drift apart.
pub const HOLDOUT: &[&str] = &[
    "scenarios/holdout/h1-light-chat.txt",
    "scenarios/holdout/h2-near-rated-chat.txt",
    "scenarios/holdout/h3-over-rated-chat.txt",
    "scenarios/holdout/h4-long-prompt-mixture.txt",
    "scenarios/holdout/h5-bursty-step.txt",
    "scenarios/holdout/h6-low-rate-heavy-context.txt",
    "scenarios/holdout/h7-code-completion.txt",
    "scenarios/holdout/h8-retry-storm.txt",
];

/// Paths of the held-out suite, resolved against `dir` (the workspace root in normal use).
pub fn holdout_suite(dir: &str) -> Vec<String> {
    HOLDOUT
        .iter()
        .map(|p| if dir.is_empty() || dir == "." { p.to_string() } else { format!("{dir}/{p}") })
        .collect()
}

/// The routing policies the engine supports today. `Routing::parse` is the authority; this list is
/// what a round enumerates when the caller does not name policies.
pub const POLICIES: &[&str] =
    &["round_robin", "random", "least_requests", "least_queue_tokens", "p2c"];

// ---------------------------------------------------------------------------------------------
// 6. A round, docs/arena.md section 4
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RoundConfig {
    pub sla_cap: f64,
    /// Per-policy declared rated capacity. Empty means every policy inherits the scenario's analytic
    /// capacity, which is the honest state of affairs until `PolicySpec` gains the field.
    pub declared_rated_rps: Vec<(String, f64)>,
    pub min_warmup_s: f64,
    /// Re-run the first pair and compare fingerprints. Section 4 step 4 asks for a determinism replay
    /// on a sample; one pair is the sample, and it costs one run.
    pub replay_check: bool,
    /// Reject a load that fails the realism envelope instead of scoring it. Section 2.2's rule, off by
    /// default for the held-out suite because that suite is frozen by hand and audited once.
    pub enforce_envelope: bool,
}

impl Default for RoundConfig {
    fn default() -> Self {
        RoundConfig {
            sla_cap: DEFAULT_SLA_CAP,
            declared_rated_rps: Vec::new(),
            min_warmup_s: WARMUP_FLOOR_TODAY_S,
            replay_check: true,
            enforce_envelope: false,
        }
    }
}

/// One entry of the payoff matrix.
#[derive(Clone, Debug)]
pub struct Cell {
    pub policy: String,
    pub load: String,
    pub run: RunScore,
}

#[derive(Clone, Debug)]
pub struct RoundResult {
    pub sla_cap: f64,
    pub policies: Vec<String>,
    pub loads: Vec<String>,
    /// Row-major, `policies.len() * loads.len()`.
    pub cells: Vec<Cell>,
    /// Ranked best first by worst-case score, ties broken by mean then by name so the order is total
    /// and reproducible.
    pub ranking: Vec<PolicyScore>,
    /// Section 4 step 6: a load is worth what it costs the *best* policy, so a load that defeats a
    /// weak policy and not a strong one is worth little. Reported as the fractional goodput shortfall
    /// the best policy suffers on that load against its own best load.
    pub load_difficulty: Vec<(String, f64)>,
    pub envelope_violations: Vec<(String, Vec<Violation>)>,
    pub warmup_violations: Vec<Violation>,
    pub determinism_checked: Option<bool>,
    pub runs: usize,
}

impl RoundResult {
    pub fn cell(&self, policy: &str, load: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.policy == policy && c.load == load)
    }
}

/// Run every (policy, load) pair and score the round.
///
/// Single-threaded, in a fixed order, with the seed taken from each scenario file and never varied by
/// policy — section 4 step 2, and the reason a difference in score is a difference in policy. The
/// scenario's own `routing` key is overridden, so the same file serves every policy.
pub fn run_round(
    policies: &[String],
    scenario_paths: &[String],
    cfg: &RoundConfig,
) -> Result<RoundResult, String> {
    let mut loads: Vec<(String, Scenario)> = Vec::new();
    let mut envelope_violations = Vec::new();
    let mut warmup_violations = Vec::new();
    for path in scenario_paths {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut sc = Scenario::parse(&text).map_err(|e| format!("{path}: {e}"))?;
        if sc.name == "unnamed" {
            sc.name = path.clone();
        }
        let vs = check_realism(&sc);
        if !vs.is_empty() {
            if cfg.enforce_envelope {
                return Err(format!(
                    "{path}: rejected by the realism envelope: {}",
                    vs.iter().map(|v| v.to_string()).collect::<Vec<_>>().join("; ")
                ));
            }
            envelope_violations.push((sc.name.clone(), vs));
        }
        if let Some(w) = check_warmup(&sc, cfg.min_warmup_s) {
            warmup_violations.push(w);
        }
        loads.push((sc.name.clone(), sc));
    }

    let mut cells: Vec<Cell> = Vec::new();
    let mut ranking: Vec<PolicyScore> = Vec::new();
    let mut runs = 0usize;
    let mut determinism_checked = None;

    for policy in policies {
        let declared = cfg
            .declared_rated_rps
            .iter()
            .find(|(p, _)| p == policy)
            .map(|(_, v)| *v);
        let scfg = ScoreConfig {
            sla_cap: cfg.sla_cap,
            declared_rated_rps: declared,
            count_failures_as_out_of_slo: true,
        };
        let mut scored: Vec<RunScore> = Vec::new();
        for (name, base) in &loads {
            let mut sc = base.clone();
            sc.routing = policy.clone();
            let result = sim::run(&sc)?;
            runs += 1;
            if cfg.replay_check && determinism_checked.is_none() {
                let again = sim::run(&sc)?;
                runs += 1;
                determinism_checked = Some(
                    again.fingerprint == result.fingerprint
                        && again.records.len() == result.records.len(),
                );
            }
            let mut rs = score_run(&result, &scfg);
            // The label the engine reports includes p2c's d, which is what a reader wants, but the
            // matrix is indexed by the name the caller passed.
            rs.policy = policy.clone();
            rs.load = name.clone();
            scored.push(rs);
        }
        for rs in &scored {
            cells.push(Cell { policy: policy.clone(), load: rs.load.clone(), run: rs.clone() });
        }
        ranking.push(score_policy(policy, scored, &scfg));
    }

    // Worst-case score first, per section 1. The remaining keys exist only to make the order total and
    // reproducible, and to stop a round in which every candidate is gated from sorting alphabetically.
    ranking.sort_by(|a, b| {
        let key = |p: &PolicyScore| {
            (
                p.score.unwrap_or(f64::NEG_INFINITY),
                p.mean_score.unwrap_or(f64::NEG_INFINITY),
                p.mean_goodput.unwrap_or(f64::NEG_INFINITY),
            )
        };
        let (a1, a2, a3) = key(a);
        let (b1, b2, b3) = key(b);
        b1.partial_cmp(&a1)
            .unwrap()
            .then(b2.partial_cmp(&a2).unwrap())
            .then(b3.partial_cmp(&a3).unwrap())
            .then(a.policy.cmp(&b.policy))
    });

    // Load difficulty against the best policy only.
    let mut load_difficulty = Vec::new();
    if let Some(best) = ranking.first() {
        let peak = best
            .runs
            .iter()
            .map(|r| r.goodput_tokens_s)
            .fold(0.0f64, f64::max);
        for (name, _) in &loads {
            let g = best
                .runs
                .iter()
                .find(|r| &r.load == name)
                .map(|r| r.goodput_tokens_s)
                .unwrap_or(0.0);
            let d = if peak > 0.0 { 1.0 - g / peak } else { 0.0 };
            load_difficulty.push((name.clone(), d));
        }
        load_difficulty.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(a.0.cmp(&b.0)));
    }

    Ok(RoundResult {
        sla_cap: cfg.sla_cap,
        policies: policies.to_vec(),
        loads: loads.iter().map(|(n, _)| n.clone()).collect(),
        cells,
        ranking,
        load_difficulty,
        envelope_violations,
        warmup_violations,
        determinism_checked,
        runs,
    })
}

/// Replay one scenario twice and compare fingerprints. Section 2.3's determinism row.
pub fn replay_is_deterministic(sc: &Scenario) -> Result<bool, String> {
    let a = sim::run(sc)?;
    let b = sim::run(sc)?;
    Ok(a.fingerprint == b.fingerprint && a.records.len() == b.records.len())
}

// ---------------------------------------------------------------------------------------------
// 7. Text output and the entry point
// ---------------------------------------------------------------------------------------------

/// The round as fixed-width text: a payoff matrix, the ranking, and every mechanical check.
pub fn round_text(r: &RoundResult) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "arena round: {} policies x {} loads, {} runs, SLA cap {:.4}",
        r.policies.len(),
        r.loads.len(),
        r.runs,
        r.sla_cap
    );
    let _ = writeln!(s);

    let w = 22;
    let short = |name: &str| -> String {
        let n: String = name.chars().take(w - 2).collect();
        n
    };

    for (title, get) in [
        (
            "goodput tokens/s within SLO (payoff matrix)",
            (|c: &Cell| format!("{:.0}", c.run.goodput_tokens_s)) as fn(&Cell) -> String,
        ),
        (
            "arena SLO attainment (all requests, sheds and timeouts included)",
            |c: &Cell| format!("{:.4}", c.run.attainment),
        ),
        (
            "engine SLO attainment (successful requests only, for contrast)",
            |c: &Cell| format!("{:.4}", c.run.engine_attainment),
        ),
        (
            "goodput as a share of offered output tokens (comparable across loads; a diagnostic)",
            |c: &Cell| format!("{:.3}", c.run.goodput_share),
        ),
        (
            "gated score: goodput if attainment >= cap else 0; '-' means out of scope",
            |c: &Cell| {
                if c.run.in_scope {
                    format!("{:.0}", c.run.score)
                } else {
                    "-".into()
                }
            },
        ),
    ] {
        let _ = writeln!(s, "{title}");
        let _ = write!(s, "{:<22}", "load");
        for p in &r.policies {
            let _ = write!(s, "{:>20}", short(p));
        }
        let _ = writeln!(s);
        for l in &r.loads {
            let _ = write!(s, "{:<22}", short(l));
            for p in &r.policies {
                let v = r.cell(p, l).map(&get).unwrap_or_else(|| "?".into());
                let _ = write!(s, "{:>20}", v);
            }
            let _ = writeln!(s);
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "offered load vs rated capacity");
    let _ = writeln!(
        s,
        "{:<22}{:>12}{:>12}{:>12}{:>10}{:>12}",
        "load", "mean rps", "peak rps", "rated rps", "in scope", "free shed"
    );
    for l in &r.loads {
        if let Some(c) = r.policies.first().and_then(|p| r.cell(p, l)) {
            let _ = writeln!(
                s,
                "{:<22}{:>12.1}{:>12.1}{:>12.1}{:>10}{:>12.3}",
                short(l),
                c.run.offered.mean_rps,
                c.run.offered.peak_rps,
                c.run.rated_rps,
                if c.run.in_scope { "yes" } else { "no" },
                c.run.free_shed_fraction
            );
        }
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "ranking (worst case over in-scope loads, section 1)");
    let _ = writeln!(
        s,
        "{:<4}{:<22}{:>12}{:>12}{:>14}{:>10}{:>6}{:>10}  {}",
        "#", "policy", "score", "mean", "mean goodput", "in scope", "out", "breaches", "worst load"
    );
    for (i, p) in r.ranking.iter().enumerate() {
        let _ = writeln!(
            s,
            "{:<4}{:<22}{:>12}{:>12}{:>14}{:>10}{:>6}{:>10}  {}",
            i + 1,
            short(&p.policy),
            p.score.map(|v| format!("{v:.0}")).unwrap_or_else(|| "none".into()),
            p.mean_score.map(|v| format!("{v:.0}")).unwrap_or_else(|| "none".into()),
            p.mean_goodput.map(|v| format!("{v:.0}")).unwrap_or_else(|| "none".into()),
            p.loads_in_scope,
            p.loads_out_of_scope,
            p.gate_breaches,
            p.worst_load.clone().unwrap_or_else(|| "-".into())
        );
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "load difficulty against the best policy (section 4 step 6)");
    for (name, d) in &r.load_difficulty {
        let _ = writeln!(s, "  {:<24}{:.3}", short(name), d);
    }
    let _ = writeln!(s);

    let _ = writeln!(s, "mechanical checks");
    let _ = writeln!(
        s,
        "  determinism replay: {}",
        match r.determinism_checked {
            Some(true) => "identical fingerprint",
            Some(false) => "MISMATCH",
            None => "not run",
        }
    );
    if r.envelope_violations.is_empty() {
        let _ = writeln!(s, "  realism envelope: every load inside the measured ranges");
    } else {
        for (load, vs) in &r.envelope_violations {
            for v in vs {
                let _ = writeln!(s, "  realism envelope: {load}: {v}");
            }
        }
    }
    if r.warmup_violations.is_empty() {
        let _ = writeln!(s, "  warm-up: adequate on every load");
    } else {
        for v in &r.warmup_violations {
            let _ = writeln!(s, "  warm-up: {v}");
        }
    }
    s
}

pub fn honest_capacity_text(h: &HonestCapacity) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "honest rated capacity for {} at cap {:.4} (analytic rated_rps {:.1})",
        h.policy, h.sla_cap, h.analytic_rps
    );
    let _ = writeln!(s, "{:>12}{:>14}{:>16}", "offered rps", "attainment", "goodput tok/s");
    for (rate, att, good) in &h.curve {
        let _ = writeln!(s, "{rate:>12.1}{att:>14.4}{good:>16.0}");
    }
    let _ = writeln!(
        s,
        "  highest passing: {}   monotone frontier: {}",
        h.highest_passing_rps.map(|v| format!("{v:.1} rps")).unwrap_or_else(|| "none".into()),
        h.monotone_rps.map(|v| format!("{v:.1} rps")).unwrap_or_else(|| "none".into())
    );
    s
}

/// Entry point for a future `sim-run arena` subcommand.
///
/// Deliberately not wired into `report::cli`: that file belongs to another change in flight. Wiring is
/// one match arm — `"arena" => lbsim::arena::arena_main(rest)`.
///
/// ```text
/// sim-run arena [--cap 0.999] [--policies a,b,c] [--capacity-sweep RATES] [scenario.txt ...]
/// ```
///
/// With no scenarios it runs the frozen held-out suite. `--capacity-sweep` measures honest rated
/// capacity for every policy on the first scenario at the given comma-separated rates instead of
/// running a round.
pub fn arena_main(args: Vec<String>) -> Result<(), String> {
    let mut cap = DEFAULT_SLA_CAP;
    let mut policies: Vec<String> = POLICIES.iter().map(|s| s.to_string()).collect();
    let mut paths: Vec<String> = Vec::new();
    let mut sweep: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cap" => {
                cap = args
                    .get(i + 1)
                    .ok_or("--cap needs a value")?
                    .parse()
                    .map_err(|_| "--cap needs a number")?;
                i += 2;
            }
            "--policies" => {
                policies = args
                    .get(i + 1)
                    .ok_or("--policies needs a list")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect();
                i += 2;
            }
            "--capacity-sweep" => {
                sweep = args
                    .get(i + 1)
                    .ok_or("--capacity-sweep needs rates")?
                    .split(',')
                    .map(|s| s.trim().parse::<f64>().map_err(|_| "rates must be numbers"))
                    .collect::<Result<_, _>>()?;
                i += 2;
            }
            other if other.starts_with("--") => return Err(format!("unexpected argument {other:?}")),
            other => {
                paths.push(other.to_string());
                i += 1;
            }
        }
    }
    if paths.is_empty() {
        paths = holdout_suite(".");
    }

    if !sweep.is_empty() {
        let text = std::fs::read_to_string(&paths[0]).map_err(|e| format!("{}: {e}", paths[0]))?;
        let base = Scenario::parse(&text)?;
        for p in &policies {
            let h = measure_honest_capacity(&base, p, &sweep, cap)?;
            println!("{}", honest_capacity_text(&h));
        }
        return Ok(());
    }

    let cfg = RoundConfig { sla_cap: cap, ..Default::default() };
    let round = run_round(&policies, &paths, &cfg)?;
    println!("{}", round_text(&round));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holdout() -> Vec<String> {
        holdout_suite(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }
    fn policies() -> Vec<String> {
        POLICIES.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn holdout_suite_parses_and_is_realistic() {
        for p in holdout() {
            let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"));
            let sc = Scenario::parse(&text).unwrap_or_else(|e| panic!("{p}: {e}"));
            let v = check_realism(&sc);
            assert!(v.is_empty(), "{p}: {:?}", v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn envelope_rejects_the_specification_s_own_example() {
        // arena.md 2.2: "A load of a million-token prompts at ten thousand requests per second is not
        // a clever attack, it is an invalid submission."
        let mut sc = Scenario::default();
        sc.arrival_rps = 10_000.0;
        sc.prompt_mean = 1_000_000.0;
        let v = check_realism(&sc);
        let keys: Vec<&str> = v.iter().map(|x| x.key.as_str()).collect();
        assert!(keys.contains(&"arrival_rps"), "{keys:?}");
        assert!(keys.contains(&"prompt_mean"), "{keys:?}");
    }

    #[test]
    fn shedding_does_not_buy_attainment() {
        // The property the arena's own attainment definition exists to enforce.
        let text = std::fs::read_to_string(&holdout()[2]).unwrap();
        let mut sc = Scenario::parse(&text).unwrap();
        sc.routing = "round_robin".into();
        let r = sim::run(&sc).unwrap();
        assert!(
            arena_attainment(&r, 0.0) <= r.slo_attainment() + 1e-9,
            "arena attainment must be no kinder than the engine's"
        );
    }

    /// The round, printed. `cargo test --release -- --nocapture arena_round` is how the numbers in
    /// `docs/arena-implementation.md` were produced.
    #[test]
    fn arena_round_over_the_holdout_suite() {
        for cap in [0.999, 0.99, 0.95] {
            let cfg = RoundConfig { sla_cap: cap, ..Default::default() };
            let round = run_round(&policies(), &holdout(), &cfg).unwrap();
            println!("{}", round_text(&round));
            assert_eq!(round.determinism_checked, Some(true));
        }
    }

    /// What every policy's honest declaration would be, at three caps, on the two mixtures that bracket
    /// the suite: h2's chat load and h7's code completion. `cargo test --release -- --nocapture
    /// honest_capacity` reproduces the tables in `docs/arena-implementation.md`.
    #[test]
    fn honest_capacity_of_every_policy() {
        let cases: [(usize, &[f64]); 2] = [
            (1, &[16.0, 33.0, 49.0, 66.0, 98.0, 118.0, 148.0, 177.0, 197.0, 246.0, 295.0]),
            (6, &[18.0, 35.0, 53.0, 71.0, 106.0, 130.0, 159.0, 190.0, 212.0, 265.0, 318.0]),
        ];
        for (idx, rates) in cases {
            let text = std::fs::read_to_string(&holdout()[idx]).unwrap();
            let base = Scenario::parse(&text).unwrap();
            for cap in [0.999, 0.99, 0.95] {
                for p in policies() {
                    let h = measure_honest_capacity(&base, &p, rates, cap).unwrap();
                    println!("{}", honest_capacity_text(&h));
                }
            }
        }
    }
}

/// How the held-out suite's offered rates were chosen, kept runnable so the choice can be audited or
/// redone against a recalibrated engine rather than taken on trust. `#[ignore]`d because it is a
/// 360-run sweep, not a test of anything: `cargo test --release -- --ignored --nocapture
/// attainment_vs_rate`.
#[cfg(test)]
mod suite_anchoring {
    use super::*;
    #[test]
    #[ignore]
    fn attainment_vs_rate() {
        for f in holdout_suite(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")) {
            let text = std::fs::read_to_string(&f).unwrap();
            let base = Scenario::parse(&text).unwrap();
            let rated = base.rated_rps();
            println!("== {} analytic {:.1} rps", base.name, rated);
            for mult in [0.05, 0.10, 0.15, 0.20, 0.30, 0.45, 0.60, 0.90, 1.4] {
                let mut line = format!("  x{:.2} = {:7.1} rps :", mult, rated * mult);
                for p in POLICIES {
                    let mut sc = base.clone();
                    sc.routing = p.to_string();
                    sc.arrival_rps = rated * mult;
                    let r = sim::run(&sc).unwrap();
                    line += &format!(" {:>7.4}", arena_attainment(&r, 0.0));
                }
                println!("{line}");
            }
        }
    }
}
