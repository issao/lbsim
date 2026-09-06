//! Scenario configuration.
//!
//! A flat `key = value` text format, parsed by hand. `docs/execution-plan.md` argues for protobuf
//! text format, and that still stands once code generation exists; today there is no generated type
//! to parse into, and a converter would be more work than the format is worth. The keys match the
//! proto field names so the eventual move is mechanical.

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
    /// Marginal step cost per decoding sequence. Stands in for the bandwidth term; the full model
    /// makes this grow with resident KV, which today's scope excludes.
    pub step_per_seq_ms: f64,
    /// Prefill is compute-bound, so it is a token rate rather than a per-sequence cost.
    pub prefill_tokens_per_s: f64,
    /// Chunked prefill budget. Bounds the stall a long prompt inflicts on everyone already decoding.
    pub step_token_budget: u32,
    /// Per-replica queue cap. Shedding here is the cheap failure.
    pub max_queue: usize,

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
    /// Step change in offered load, used to drive a collapse and then test recovery.
    pub load_step_at_s: f64,
    pub load_step_factor: f64,
    pub load_step_until_s: f64,

    // -- routing -----------------------------------------------------------
    pub routing: String,
    pub p2c_choices: usize,
    /// Pay a modelled probe for fresh state instead of reading the delayed snapshot, so the cost of
    /// freshness is visible rather than free.
    pub probe_live: bool,

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
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: "unnamed".into(),
            seed: 1,
            duration_s: 60.0,
            warmup_s: 5.0,
            replicas: 32,
            max_batch: 32,
            step_base_ms: 2.75,
            step_per_seq_ms: 0.25,
            prefill_tokens_per_s: 25_000.0,
            step_token_budget: 2048,
            max_queue: 64,
            arrival_rps: 40.0,
            prompt_mean: 1200.0,
            prompt_cv: 1.2,
            output_mean: 300.0,
            output_cv: 1.5,
            long_probability: 0.08,
            long_prompt_mean: 24_000.0,
            long_output_mean: 400.0,
            load_step_at_s: -1.0,
            load_step_factor: 1.0,
            load_step_until_s: -1.0,
            routing: "round_robin".into(),
            p2c_choices: 2,
            probe_live: false,
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
        for (k, v) in &kv {
            let f = |d: &str| -> f64 { v.parse::<f64>().unwrap_or_else(|_| panic!("{} is not a number in {}", v, d)) };
            match k.as_str() {
                "name" => s.name = v.clone(),
                "seed" => s.seed = v.parse().map_err(|_| "seed")?,
                "duration_s" => s.duration_s = f("duration_s"),
                "warmup_s" => s.warmup_s = f("warmup_s"),
                "replicas" => s.replicas = f("replicas") as usize,
                "max_batch" => s.max_batch = f("max_batch") as usize,
                "step_base_ms" => s.step_base_ms = f("step_base_ms"),
                "step_per_seq_ms" => s.step_per_seq_ms = f("step_per_seq_ms"),
                "prefill_tokens_per_s" => s.prefill_tokens_per_s = f("prefill_tokens_per_s"),
                "step_token_budget" => s.step_token_budget = f("step_token_budget") as u32,
                "max_queue" => s.max_queue = f("max_queue") as usize,
                "arrival_rps" => s.arrival_rps = f("arrival_rps"),
                "prompt_mean" => s.prompt_mean = f("prompt_mean"),
                "prompt_cv" => s.prompt_cv = f("prompt_cv"),
                "output_mean" => s.output_mean = f("output_mean"),
                "output_cv" => s.output_cv = f("output_cv"),
                "long_probability" => s.long_probability = f("long_probability"),
                "long_prompt_mean" => s.long_prompt_mean = f("long_prompt_mean"),
                "long_output_mean" => s.long_output_mean = f("long_output_mean"),
                "load_step_at_s" => s.load_step_at_s = f("load_step_at_s"),
                "load_step_factor" => s.load_step_factor = f("load_step_factor"),
                "load_step_until_s" => s.load_step_until_s = f("load_step_until_s"),
                "routing" => s.routing = v.clone(),
                "p2c_choices" => s.p2c_choices = f("p2c_choices") as usize,
                "probe_live" => s.probe_live = v == "true",
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
                other => unknown.push(other.to_string()),
            }
        }
        if !unknown.is_empty() {
            return Err(format!("unknown keys: {}", unknown.join(", ")));
        }
        Ok(s)
    }

    /// Rated capacity, in requests per second, from the cost model rather than from a guess.
    ///
    /// Prefill and decode contend for the same device, so device-seconds per request are additive.
    /// This is the number `docs/ARCHITECTURE.md` section 1.2 got wrong by quoting decode-only
    /// throughput, which Issao caught.
    pub fn rated_rps(&self) -> f64 {
        let p_mean = self.prompt_mean * (1.0 - self.long_probability)
            + self.long_prompt_mean * self.long_probability;
        let o_mean = self.output_mean * (1.0 - self.long_probability)
            + self.long_output_mean * self.long_probability;
        let step_s = (self.step_base_ms + self.step_per_seq_ms * self.max_batch as f64) / 1000.0;
        let prefill_s = p_mean / self.prefill_tokens_per_s;
        let decode_s = o_mean * step_s / self.max_batch as f64;
        self.replicas as f64 / (prefill_s + decode_s)
    }

    pub fn to_text(&self) -> String {
        format!(
            "name = {}\nseed = {}\nduration_s = {}\nwarmup_s = {}\nreplicas = {}\nmax_batch = {}\n\
             step_base_ms = {}\nstep_per_seq_ms = {}\nprefill_tokens_per_s = {}\n\
             step_token_budget = {}\nmax_queue = {}\narrival_rps = {}\nprompt_mean = {}\n\
             prompt_cv = {}\noutput_mean = {}\noutput_cv = {}\nlong_probability = {}\n\
             long_prompt_mean = {}\nlong_output_mean = {}\nload_step_at_s = {}\n\
             load_step_factor = {}\nload_step_until_s = {}\nrouting = {}\np2c_choices = {}\n\
             probe_live = {}\ntelemetry_interval_ms = {}\ntelemetry_delay_ms = {}\n\
             client_timeout_s = {}\nmax_attempts = {}\nretry_budget_fraction = {}\n\
             retry_backoff_s = {}\nttft_slo_ms = {}\nitl_slo_ms = {}\ne2e_slo_s = {}\n\
             sample_interval_ms = {}\n",
            self.name, self.seed, self.duration_s, self.warmup_s, self.replicas, self.max_batch,
            self.step_base_ms, self.step_per_seq_ms, self.prefill_tokens_per_s,
            self.step_token_budget, self.max_queue, self.arrival_rps, self.prompt_mean,
            self.prompt_cv, self.output_mean, self.output_cv, self.long_probability,
            self.long_prompt_mean, self.long_output_mean, self.load_step_at_s,
            self.load_step_factor, self.load_step_until_s, self.routing, self.p2c_choices,
            self.probe_live, self.telemetry_interval_ms, self.telemetry_delay_ms,
            self.client_timeout_s, self.max_attempts, self.retry_budget_fraction,
            self.retry_backoff_s, self.ttft_slo_ms, self.itl_slo_ms, self.e2e_slo_s,
            self.sample_interval_ms
        )
    }
}
