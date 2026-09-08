//! Scenario configuration.
//!
//! A flat `key = value` text format, parsed by hand. `docs/execution-plan.md` argues for protobuf
//! text format, and that still stands once code generation exists; today there is no generated type
//! to parse into, and a converter would be more work than the format is worth. The keys match the
//! proto field names so the eventual move is mechanical.

use sim_physics::CostModel;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: String,
    pub seed: u64,
    pub duration_s: f64,
    pub warmup_s: f64,

    // -- fleet -------------------------------------------------------------
    pub replicas: usize,
    pub max_batch: usize,
    /// Fixed per-step cost: launch, sampling, scheduler. Dominates at low batch, and measured
    /// batch-1 decode is well above the roofline because of it. Calibrated to 2.75 ms in
    /// `bench/validate_epochs.py` against a published measurement.
    pub step_base_ms: f64,
    /// Marginal step cost per decoding sequence: sampling and bookkeeping. Small, and distinct from
    /// the bandwidth term below.
    pub step_per_seq_ms: f64,
    /// Step cost per thousand resident key-value tokens.
    ///
    /// This is the memory-bandwidth term, and it is what makes step time grow as contexts lengthen
    /// rather than only as the batch widens. For a 70B model on 8xH100 it is
    /// `kv_bytes_per_token / (tp * hbm_bandwidth * mbu)`, which works out at about 0.0175 ms per
    /// thousand tokens. With `step_base_ms` covering the weight read plus fixed overhead, the pair
    /// reproduces `bench/validate_epochs.py` to within 0.05 ms at every batch size in its table.
    pub step_per_kv_ktoken_ms: f64,
    /// Key-value cache capacity per replica, in tokens. **The real capacity constraint.**
    ///
    /// Capacity is a token budget, not a request count: one 24,000-token context costs what eight
    /// 3,000-token chat turns cost. A replica admits only while resident tokens allow, so a long
    /// prompt can block admission that a request count would have permitted.
    pub kv_capacity_tokens: f64,
    /// Prefill is compute-bound, so it is a token rate rather than a per-sequence cost.
    pub prefill_tokens_per_s: f64,
    /// Chunked prefill budget. Bounds the stall a long prompt inflicts on everyone already decoding.
    pub step_token_budget: u32,
    /// Per-replica queue cap. Shedding here is the cheap failure.
    pub max_queue: usize,
    /// The knob VISION.md section 3a asks for "early on": with decode's bandwidth term zeroed, traffic
    /// behaves like stateless serving and the rolling hotspot can be shown without any LLM physics.
    pub disable_decode: bool,
    /// What a replica does when resident context outgrows its cache: `never` (a blocked request
    /// waits, and a parked session's context stays until its next turn), `recompute` (drop the
    /// victim's context and prefill it again on re-admission), `swap_to_dram` (copy it over the host
    /// link and back, paying the transfer instead of the compute), or `swap_else_recompute` (swap
    /// while `dram_capacity_tokens` has room, recompute once it has not).
    pub preemption: String,
    /// Which resident context goes first: `newest` (last admitted, what vLLM does), `largest_kv`, or
    /// `latest_deadline` (the request with the most slack).
    pub preemption_victim: String,
    /// Host-side room for swapped context per replica, in tokens. Zero means four times the cache.
    pub dram_capacity_tokens: f64,
    /// Host link bandwidth for swapping context, in GB/s.
    pub swap_gbps: f64,

    // -- workload ----------------------------------------------------------
    pub arrival_rps: f64,
    pub prompt_mean: f64,
    pub prompt_cv: f64,
    pub output_mean: f64,
    pub output_cv: f64,
    /// Traffic is a mixture, and it is the mixture that creates head-of-line blocking. A unimodal
    /// workload hides the phenomenon being studied.
    pub long_probability: f64,
    pub long_prompt_mean: f64,
    pub long_output_mean: f64,
    /// Multi-turn sessions. A request that completes may be followed by another turn from the same
    /// conversation after `session_think_s`, carrying the whole previous context plus a short new
    /// prompt, and that context stays resident on the replica in between. The number of turns is
    /// geometric with this mean; one means every request is its own conversation.
    pub session_turns_mean: f64,
    pub session_think_s: f64,
    /// Step change in offered load, used to drive a collapse and then test recovery.
    pub load_step_at_s: f64,
    pub load_step_factor: f64,
    pub load_step_until_s: f64,

    // -- policies ------------------------------------------------------------
    // Names resolve through the registries in `sim_policy`; an unknown name is an error at run start.
    pub routing: String,
    pub p2c_choices: usize,
    /// Pay a modelled probe for fresh state instead of reading the delayed snapshot, so the cost of
    /// freshness is visible rather than free.
    pub probe_live: bool,
    pub admission: String,
    /// For deadline-aware admission: the fraction of a request's deadline reserved for serving it, so
    /// a request that would spend more than the rest waiting is shed before it costs anything.
    pub admission_headroom: f64,
    /// For weighted fair share: how far a tenant may burst above its share before being shed.
    pub fair_share_burst: f64,

    // -- tenants ---------------------------------------------------------------
    /// How many tenants share the fleet. One means no tenancy at all.
    pub tenants: usize,
    /// Relative entitlements, one per tenant; empty means equal. This is the fair share a tenant may
    /// claim, and it is deliberately separate from what it *sends*: a fair-share policy only has work
    /// to do when some tenant offers more than its share.
    pub tenant_weights: Vec<f64>,
    /// Relative offered load, one per tenant; empty means the same as `tenant_weights`, so a scenario
    /// that says nothing has every tenant sending exactly its share.
    pub tenant_demand: Vec<f64>,

    // -- telemetry ---------------------------------------------------------
    pub telemetry_interval_ms: f64,
    pub telemetry_delay_ms: f64,

    // -- client ------------------------------------------------------------
    pub client_timeout_s: f64,
    pub max_attempts: u32,
    /// Cap on the fraction of traffic that may be retries. Without it a slowdown becomes a retry
    /// storm and then a collapse the fleet does not recover from.
    pub retry_budget_fraction: f64,
    pub retry_backoff_s: f64,

    // -- SLOs --------------------------------------------------------------
    pub ttft_slo_ms: f64,
    pub itl_slo_ms: f64,
    pub e2e_slo_s: f64,

    pub sample_interval_ms: f64,
    /// Fraction of arrivals whose journey is recorded span by span, drawn from a stream of its own so
    /// the run is byte-identical with tracing on or off. Zero records nothing.
    pub trace_sample_rate: f64,

    // -- trace replay --------------------------------------------------------
    /// `synthetic` draws arrivals and shapes from the distributions above; `trace` replays
    /// `trace_file` row by row and touches no random stream, so a recorded burst can be re-run under
    /// another policy with nothing else changed.
    pub workload: String,
    /// CSV `t_s,prompt_tokens,output_tokens,tenant` with a header line, relative to the process cwd
    /// like every other scenario path. Only read when `workload = trace`.
    pub trace_file: String,

    // -- failures ------------------------------------------------------------
    /// Failure events, `;`-separated: `t=<s>,replica=<i>,kind=<crash|slow=<mult>|hang>[,until=<s>]`.
    /// Kept as text so the scenario round-trips byte for byte; `failure_events` is the parsed form.
    /// A crash is announced through telemetry after the usual delay; a slowdown or a hang is not
    /// announced at all, which is what makes it a gray failure.
    pub failures: String,
    // -- speculative decoding ------------------------------------------------
    /// Draft tokens a small model proposes per step, verified by the big model in that same step. Zero
    /// is off. VISION section 3a: "observe the value and cost of speculative decoding".
    pub spec_draft_tokens: u32,
    /// Probability the big model accepts each draft token, so a sequence advances an expected
    /// (1 - a^(N+1)) / (1 - a) tokens a step. The formula is `CostModel::spec_tokens_per_step`.
    pub spec_accept_rate: f64,
    // -- SLO classes ---------------------------------------------------------
    /// Class shares of arrivals, `interactive:0.7,agent:0.2,batch:0.1`. Empty means no classes: the
    /// `*_slo_*` keys above apply to everyone. With classes on, each request is judged against its
    /// class's fixed targets (see `SloClass`) and those keys are ignored, because a fleet serving an
    /// agent loop and a batch job is not one SLO with two workloads, it is two SLOs.
    pub slo_classes: String,
}

/// The per-class service targets. Constants rather than keys: the classes are a vocabulary shared
/// across scenarios so that "batch attainment" means the same thing in every report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SloClass {
    pub name: &'static str,
    pub ttft_slo_ms: f64,
    /// Infinite for a class with no inter-token target: a batch job cares when it finishes, not
    /// how evenly.
    pub itl_slo_ms: f64,
    pub e2e_slo_s: f64,
}

/// Class ids are fixed by this table, not by position in `slo_classes`, so a record's class means the
/// same thing whichever subset a scenario enables. Id 0 is reserved for "no class".
pub const SLO_CLASSES: [SloClass; 3] = [
    SloClass { name: "interactive", ttft_slo_ms: 2000.0, itl_slo_ms: 80.0, e2e_slo_s: 30.0 },
    SloClass { name: "agent", ttft_slo_ms: 5000.0, itl_slo_ms: 150.0, e2e_slo_s: 30.0 },
    SloClass { name: "batch", ttft_slo_ms: 60_000.0, itl_slo_ms: f64::INFINITY, e2e_slo_s: 60.0 },
];

/// Parses `name:share,...` into `(class id, share)` pairs, unnormalised. `Err` names the bad part.
pub fn parse_slo_classes(text: &str) -> Result<Vec<(u8, f64)>, String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, share) = part.split_once(':').ok_or_else(|| format!("{part:?} is not name:share"))?;
        let id = SLO_CLASSES
            .iter()
            .position(|c| c.name == name.trim())
            .ok_or_else(|| format!("{name:?} is not an SLO class"))?;
        let share: f64 = share.trim().parse().map_err(|_| format!("{part:?}: share is not a number"))?;
        if share < 0.0 {
            return Err(format!("{part:?}: share is negative"));
        }
        out.push((id as u8 + 1, share));
    }
    Ok(out)
}

/// What a live change to a key means for a run that is under way.
///
/// The workload reads the scenario on every draw, so a workload key takes effect at the next arrival.
/// A policy is built once from the scenario, so a policy key rebuilds it. Everything else is fixed
/// at construction (fleet shape, physics, clocks, seed, telemetry cadence) and needs a restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideKind {
    Workload,
    Policy,
    Structural,
}

/// What goes wrong with one replica, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct FailureEvent {
    pub at: f64,
    pub replica: usize,
    pub kind: FailureKind,
    pub until: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FailureKind {
    /// Refuses everything and drops what it held; telemetry reports it ejected once the delay passes.
    Crash,
    /// Every step takes `1/mult` of its modelled time. Nothing announces it.
    Slow(f64),
    /// Steps never complete. Nothing announces it; only client timeouts notice.
    Hang,
}

/// The parsed form of `Scenario::failures`, or the first thing wrong with it.
pub fn parse_failures(text: &str) -> Result<Vec<FailureEvent>, String> {
    let mut out = Vec::new();
    for ev in text.split(';').map(str::trim).filter(|e| !e.is_empty()) {
        let (mut at, mut replica, mut kind, mut until) = (None, None, None, None);
        for field in ev.split(',').map(str::trim).filter(|f| !f.is_empty()) {
            let (k, v) = field.split_once('=').ok_or_else(|| format!("failure field {field:?} has no '='"))?;
            let (k, v) = (k.trim(), v.trim());
            let num = |what: &str| -> Result<f64, String> {
                v.parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0)
                    .ok_or_else(|| format!("failure {what} = {v:?} is not a non-negative number"))
            };
            match k {
                "t" => at = Some(num("t")?),
                "replica" => replica = Some(num("replica")? as usize),
                "until" => until = Some(num("until")?),
                "kind" => {
                    kind = Some(match v {
                        "crash" => FailureKind::Crash,
                        "hang" => FailureKind::Hang,
                        _ => match v.strip_prefix("slow=").and_then(|m| m.parse::<f64>().ok()) {
                            Some(m) if m.is_finite() && m > 0.0 => FailureKind::Slow(m),
                            _ => return Err(format!("failure kind {v:?} is not crash, hang or slow=<mult>")),
                        },
                    })
                }
                _ => return Err(format!("failure field {k:?} is not t, replica, kind or until")),
            }
        }
        let missing = |what| format!("failure {ev:?} has no {what}");
        let at = at.ok_or_else(|| missing("t"))?;
        if let Some(u) = until.filter(|u| *u <= at) {
            return Err(format!("failure {ev:?} ends at {u} before it starts at {at}"));
        }
        out.push(FailureEvent {
            at,
            replica: replica.ok_or_else(|| missing("replica"))?,
            kind: kind.ok_or_else(|| missing("kind"))?,
            until,
        });
    }
    Ok(out)
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: "unnamed".into(),
            seed: 1,
            duration_s: 60.0,
            warmup_s: 5.0,
            replicas: 256,
            max_batch: 32,
            // Weight read for a 70B model on 8xH100 at 70% bandwidth utilization, 7.46 ms, plus
            // 2.75 ms of fixed overhead calibrated against a published batch-1 measurement.
            step_base_ms: 10.2,
            step_per_seq_ms: 0.0,
            step_per_kv_ktoken_ms: 0.0175,
            kv_capacity_tokens: 1_370_000.0,
            prefill_tokens_per_s: 28_286.0,
            // A full prefill chunk costs step_base_ms + budget / prefill_tokens_per_s, and every
            // sequence decoding on that replica sees the whole step as a gap between its tokens. At
            // 2048 that is 82.6 ms against an 80 ms target, so the defaults were mutually
            // inconsistent: no amount of spare capacity or better routing could have met the SLO,
            // because the floor on achievable inter-token latency was above it. 1024 gives 46.4 ms.
            step_token_budget: 1024,
            max_queue: 64,
            disable_decode: false,
            preemption: "never".into(),
            preemption_victim: "newest".into(),
            dram_capacity_tokens: 0.0,
            swap_gbps: 50.0,
            arrival_rps: 320.0,
            prompt_mean: 1200.0,
            prompt_cv: 1.2,
            output_mean: 300.0,
            output_cv: 1.5,
            long_probability: 0.08,
            long_prompt_mean: 24_000.0,
            long_output_mean: 400.0,
            session_turns_mean: 1.0,
            session_think_s: 0.0,
            load_step_at_s: -1.0,
            load_step_factor: 1.0,
            load_step_until_s: -1.0,
            routing: "round_robin".into(),
            p2c_choices: 2,
            probe_live: false,
            admission: "accept_all".into(),
            admission_headroom: 0.5,
            fair_share_burst: 2.0,
            tenants: 1,
            tenant_weights: Vec::new(),
            tenant_demand: Vec::new(),
            telemetry_interval_ms: 1000.0,
            telemetry_delay_ms: 200.0,
            client_timeout_s: 30.0,
            max_attempts: 1,
            retry_budget_fraction: 1.0,
            retry_backoff_s: 0.5,
            ttft_slo_ms: 2000.0,
            itl_slo_ms: 80.0,
            e2e_slo_s: 30.0,
            sample_interval_ms: 250.0,
            trace_sample_rate: 0.0,
            workload: "synthetic".into(),
            trace_file: String::new(),
            spec_draft_tokens: 0,
            spec_accept_rate: 0.0,
            slo_classes: String::new(),
            failures: String::new(),
        }
    }
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Scenario, String> {
        let mut kv: BTreeMap<String, String> = BTreeMap::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected key = value, got {:?}", n + 1, raw))?;
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
        let mut s = Scenario::default();
        // Every key is optional; defaults are the reference config. An unknown key is an error
        // rather than a silent no-op, because a typo in a scenario is otherwise invisible and the
        // run quietly measures something else.
        let mut unknown = Vec::new();
        let mut malformed = Vec::new();
        for (k, v) in &kv {
            // Returns 0.0 and records the key on a bad value rather than panicking. `parse` hands back
            // a Result and carefully errors on an unknown key, so panicking on a malformed *value* was
            // the same class of typo with a wildly different failure mode, and a caller embedding this
            // in a sweep runner or a request handler cannot contain a panic.
            let mut f = |d: &str| -> f64 {
                match v.parse::<f64>() {
                    Ok(x) => x,
                    Err(_) => {
                        malformed.push(format!("{d} = {v:?}"));
                        0.0
                    }
                }
            };
            match k.as_str() {
                "name" => s.name = v.clone(),
                "seed" => match v.parse() {
                    Ok(x) => s.seed = x,
                    Err(_) => malformed.push(format!("seed = {v:?}")),
                },
                "duration_s" => s.duration_s = f("duration_s"),
                "warmup_s" => s.warmup_s = f("warmup_s"),
                "replicas" => s.replicas = f("replicas") as usize,
                "max_batch" => s.max_batch = f("max_batch") as usize,
                "step_base_ms" => s.step_base_ms = f("step_base_ms"),
                "step_per_seq_ms" => s.step_per_seq_ms = f("step_per_seq_ms"),
                "step_per_kv_ktoken_ms" => s.step_per_kv_ktoken_ms = f("step_per_kv_ktoken_ms"),
                "kv_capacity_tokens" => s.kv_capacity_tokens = f("kv_capacity_tokens"),
                "prefill_tokens_per_s" => s.prefill_tokens_per_s = f("prefill_tokens_per_s"),
                "step_token_budget" => s.step_token_budget = f("step_token_budget") as u32,
                "max_queue" => s.max_queue = f("max_queue") as usize,
                "disable_decode" => s.disable_decode = v == "true",
                "spec_draft_tokens" => s.spec_draft_tokens = f("spec_draft_tokens") as u32,
                "spec_accept_rate" => s.spec_accept_rate = f("spec_accept_rate"),
                "preemption" => s.preemption = v.clone(),
                "preemption_victim" => s.preemption_victim = v.clone(),
                "dram_capacity_tokens" => s.dram_capacity_tokens = f("dram_capacity_tokens"),
                "swap_gbps" => s.swap_gbps = f("swap_gbps"),
                "trace_sample_rate" => s.trace_sample_rate = f("trace_sample_rate"),
                "arrival_rps" => s.arrival_rps = f("arrival_rps"),
                "prompt_mean" => s.prompt_mean = f("prompt_mean"),
                "prompt_cv" => s.prompt_cv = f("prompt_cv"),
                "output_mean" => s.output_mean = f("output_mean"),
                "output_cv" => s.output_cv = f("output_cv"),
                "long_probability" => s.long_probability = f("long_probability"),
                "long_prompt_mean" => s.long_prompt_mean = f("long_prompt_mean"),
                "long_output_mean" => s.long_output_mean = f("long_output_mean"),
                "session_turns_mean" => s.session_turns_mean = f("session_turns_mean"),
                "session_think_s" => s.session_think_s = f("session_think_s"),
                "load_step_at_s" => s.load_step_at_s = f("load_step_at_s"),
                "load_step_factor" => s.load_step_factor = f("load_step_factor"),
                "load_step_until_s" => s.load_step_until_s = f("load_step_until_s"),
                "routing" => s.routing = v.clone(),
                "p2c_choices" => s.p2c_choices = f("p2c_choices") as usize,
                "probe_live" => s.probe_live = v == "true",
                "admission" => s.admission = v.clone(),
                "admission_headroom" => s.admission_headroom = f("admission_headroom"),
                "fair_share_burst" => s.fair_share_burst = f("fair_share_burst"),
                "tenants" => s.tenants = f("tenants") as usize,
                "tenant_weights" | "tenant_demand" => {
                    let mut ws = Vec::new();
                    for part in v.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                        match part.parse::<f64>() {
                            Ok(x) => ws.push(x),
                            Err(_) => malformed.push(format!("{k} = {v:?}")),
                        }
                    }
                    if k == "tenant_weights" {
                        s.tenant_weights = ws;
                    } else {
                        s.tenant_demand = ws;
                    }
                }
                "telemetry_interval_ms" => s.telemetry_interval_ms = f("telemetry_interval_ms"),
                "telemetry_delay_ms" => s.telemetry_delay_ms = f("telemetry_delay_ms"),
                "client_timeout_s" => s.client_timeout_s = f("client_timeout_s"),
                "max_attempts" => s.max_attempts = f("max_attempts") as u32,
                "retry_budget_fraction" => s.retry_budget_fraction = f("retry_budget_fraction"),
                "retry_backoff_s" => s.retry_backoff_s = f("retry_backoff_s"),
                "ttft_slo_ms" => s.ttft_slo_ms = f("ttft_slo_ms"),
                "itl_slo_ms" => s.itl_slo_ms = f("itl_slo_ms"),
                "e2e_slo_s" => s.e2e_slo_s = f("e2e_slo_s"),
                "sample_interval_ms" => s.sample_interval_ms = f("sample_interval_ms"),
                "workload" => s.workload = v.clone(),
                "trace_file" => s.trace_file = v.clone(),
                "slo_classes" => match parse_slo_classes(v) {
                    Ok(_) => s.slo_classes = v.clone(),
                    Err(e) => malformed.push(format!("{k} = {v:?} ({e})")),
                },
                "failures" => match parse_failures(v) {
                    Ok(_) => s.failures = v.clone(),
                    Err(why) => malformed.push(format!("{k} = {v:?} ({why})")),
                },
                other => unknown.push(other.to_string()),
            }
        }
        if !unknown.is_empty() || !malformed.is_empty() {
            let mut parts = Vec::new();
            if !unknown.is_empty() {
                parts.push(format!("unknown keys: {}", unknown.join(", ")));
            }
            if !malformed.is_empty() {
                parts.push(format!("values that are not numbers: {}", malformed.join(", ")));
            }
            return Err(parts.join("; "));
        }
        Ok(s)
    }

    /// The cost-model constants, for `sim_physics`. The formulas live there; this only carries the
    /// numbers across.
    pub fn cost_model(&self) -> CostModel {
        CostModel {
            step_base_ms: self.step_base_ms,
            step_per_seq_ms: self.step_per_seq_ms,
            step_per_kv_ktoken_ms: self.step_per_kv_ktoken_ms,
            prefill_tokens_per_s: self.prefill_tokens_per_s,
            disable_decode: self.disable_decode,
            swap_gbps: self.swap_gbps,
            spec_draft_tokens: self.spec_draft_tokens,
            spec_accept_rate: self.spec_accept_rate,
        }
    }

    /// Host-side room for swapped context per replica, in tokens: the key, or four times the cache
    /// when the scenario says nothing.
    pub fn dram_capacity_tokens(&self) -> f64 {
        if self.dram_capacity_tokens > 0.0 {
            self.dram_capacity_tokens
        } else {
            4.0 * self.kv_capacity_tokens
        }
    }

    /// The failure schedule, parsed. `parse` has already rejected a malformed one, so this only fails
    /// for a scenario built in code.
    pub fn failure_events(&self) -> Result<Vec<FailureEvent>, String> {
        parse_failures(&self.failures)
    }

    /// Tenant weights normalised to sum to one, one per tenant. Missing weights are one; extra weights
    /// are ignored. Empty when there is a single tenant, which is what "no tenancy" means downstream.
    pub fn tenant_shares(&self) -> Vec<f64> {
        self.normalised(&self.tenant_weights)
    }

    /// Offered-load shares, normalised; falls back to the entitlements when `tenant_demand` is empty.
    pub fn tenant_demand_shares(&self) -> Vec<f64> {
        if self.tenant_demand.is_empty() {
            self.tenant_shares()
        } else {
            self.normalised(&self.tenant_demand)
        }
    }

    fn normalised(&self, weights: &[f64]) -> Vec<f64> {
        if self.tenants <= 1 {
            return Vec::new();
        }
        let raw: Vec<f64> =
            (0..self.tenants).map(|i| weights.get(i).copied().unwrap_or(1.0).max(0.0)).collect();
        let total: f64 = raw.iter().sum();
        if total <= 0.0 {
            return vec![1.0 / self.tenants as f64; self.tenants];
        }
        raw.iter().map(|w| w / total).collect()
    }

    /// Mean prompt and output length over the two-mode mixture.
    pub fn mixture_means(&self) -> (f64, f64) {
        let p_mean = self.prompt_mean * (1.0 - self.long_probability)
            + self.long_prompt_mean * self.long_probability;
        let o_mean = self.output_mean * (1.0 - self.long_probability)
            + self.long_output_mean * self.long_probability;
        (p_mean, o_mean)
    }

    /// Rated capacity, in requests per second, from the cost model rather than from a guess.
    ///
    /// This is the number `docs/ARCHITECTURE.md` section 1.2 got wrong by quoting decode-only
    /// throughput, which Issao caught. The formula is `CostModel::rated_rps`.
    pub fn rated_rps(&self) -> f64 {
        let (p_mean, o_mean) = self.mixture_means();
        self.cost_model()
            .rated_rps(self.replicas, self.kv_capacity_tokens, self.max_batch, p_mean, o_mean)
    }

    /// Effective batch limit: the sequence cap, or the token budget, whichever binds first. Reported
    /// so it is visible which one actually constrains a scenario.
    pub fn effective_batch(&self) -> f64 {
        let (p_mean, o_mean) = self.mixture_means();
        let ctx_mean = p_mean + o_mean / 2.0;
        self.cost_model().effective_batch(self.kv_capacity_tokens, self.max_batch, ctx_mean)
    }

    /// `self` with one key set, through the text form so there is exactly one place that knows the
    /// key names. An unknown key is an error that names it; a malformed value is `parse`'s error.
    pub fn with_override(&self, key: &str, value: &str) -> Result<Scenario, String> {
        let mut replaced = false;
        let text: Vec<String> = self
            .to_text()
            .lines()
            .map(|l| {
                if l.split('=').next().map(str::trim) == Some(key) {
                    replaced = true;
                    format!("{key} = {value}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        if !replaced {
            return Err(format!("unknown key {key:?}"));
        }
        Scenario::parse(&text.join("\n"))
    }

    /// Whether a running engine can take a change to `key` without a restart. Unknown keys are
    /// structural: the safe answer for a name nothing recognises.
    pub fn override_kind(key: &str) -> OverrideKind {
        match key {
            "arrival_rps" | "load_step_at_s" | "load_step_factor" | "load_step_until_s"
            | "prompt_mean" | "prompt_cv" | "output_mean" | "output_cv"
            | "long_probability" | "long_prompt_mean" | "long_output_mean"
            | "session_turns_mean" | "session_think_s" | "tenant_demand"
            | "client_timeout_s" | "max_attempts" | "retry_budget_fraction" | "retry_backoff_s" => {
                OverrideKind::Workload
            }
            "routing" | "p2c_choices" | "probe_live" | "admission" | "admission_headroom"
            | "fair_share_burst" | "preemption" | "preemption_victim" => OverrideKind::Policy,
            _ => OverrideKind::Structural,
        }
    }

    /// The targets a request of `class` is judged against: the class table when classes are on, the
    /// scenario's own keys for class 0. Returned as `(ttft_ms, itl_ms, e2e_s)`.
    pub fn slo_for(&self, class: u8) -> (f64, f64, f64) {
        match SLO_CLASSES.get((class as usize).wrapping_sub(1)) {
            Some(c) => (c.ttft_slo_ms, c.itl_slo_ms, c.e2e_slo_s),
            None => (self.ttft_slo_ms, self.itl_slo_ms, self.e2e_slo_s),
        }
    }

    /// Normalised `(class id, share)` pairs; empty when classes are off. `parse` already rejected a
    /// malformed string, so a bad one here can only come from a struct built by hand, and it is
    /// treated as off rather than as a panic in the workload loop.
    pub fn slo_class_shares(&self) -> Vec<(u8, f64)> {
        let raw = parse_slo_classes(&self.slo_classes).unwrap_or_default();
        let total: f64 = raw.iter().map(|(_, w)| w).sum();
        if total <= 0.0 {
            return Vec::new();
        }
        raw.into_iter().map(|(c, w)| (c, w / total)).collect()
    }

    pub fn to_text(&self) -> String {
        format!(
            "name = {}\nseed = {}\nduration_s = {}\nwarmup_s = {}\nreplicas = {}\nmax_batch = {}\n\
             step_base_ms = {}\nstep_per_seq_ms = {}\nstep_per_kv_ktoken_ms = {}\n\
             kv_capacity_tokens = {}\nprefill_tokens_per_s = {}\n\
             step_token_budget = {}\nmax_queue = {}\ndisable_decode = {}\npreemption = {}\n\
             preemption_victim = {}\ndram_capacity_tokens = {}\nswap_gbps = {}\narrival_rps = {}\nprompt_mean = {}\n\
             prompt_cv = {}\noutput_mean = {}\noutput_cv = {}\nlong_probability = {}\n\
             long_prompt_mean = {}\nlong_output_mean = {}\nsession_turns_mean = {}\n\
             session_think_s = {}\nload_step_at_s = {}\n\
             load_step_factor = {}\nload_step_until_s = {}\nrouting = {}\np2c_choices = {}\n\
             probe_live = {}\nadmission = {}\nadmission_headroom = {}\nfair_share_burst = {}\n\
             tenants = {}\ntenant_weights = {}\ntenant_demand = {}\n\
             telemetry_interval_ms = {}\ntelemetry_delay_ms = {}\n\
             client_timeout_s = {}\nmax_attempts = {}\nretry_budget_fraction = {}\n\
             retry_backoff_s = {}\nttft_slo_ms = {}\nitl_slo_ms = {}\ne2e_slo_s = {}\n\
             sample_interval_ms = {}\ntrace_sample_rate = {}\nworkload = {}\ntrace_file = {}\n\
             spec_draft_tokens = {}\nspec_accept_rate = {}\nslo_classes = {}\n\
             failures = {}\n",
            self.name, self.seed, self.duration_s, self.warmup_s, self.replicas, self.max_batch,
            self.step_base_ms, self.step_per_seq_ms, self.step_per_kv_ktoken_ms,
            self.kv_capacity_tokens, self.prefill_tokens_per_s,
            self.step_token_budget, self.max_queue, self.disable_decode, self.preemption,
            self.preemption_victim, self.dram_capacity_tokens, self.swap_gbps, self.arrival_rps, self.prompt_mean,
            self.prompt_cv, self.output_mean, self.output_cv, self.long_probability,
            self.long_prompt_mean, self.long_output_mean, self.session_turns_mean,
            self.session_think_s, self.load_step_at_s,
            self.load_step_factor, self.load_step_until_s, self.routing, self.p2c_choices,
            self.probe_live, self.admission, self.admission_headroom, self.fair_share_burst,
            self.tenants,
            self.tenant_weights.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(","),
            self.tenant_demand.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(","),
            self.telemetry_interval_ms, self.telemetry_delay_ms,
            self.client_timeout_s, self.max_attempts, self.retry_budget_fraction,
            self.retry_backoff_s, self.ttft_slo_ms, self.itl_slo_ms, self.e2e_slo_s,
            self.sample_interval_ms, self.trace_sample_rate, self.workload, self.trace_file,
            self.spec_draft_tokens, self.spec_accept_rate, self.slo_classes,
            self.failures
        )
    }
}
