//! Arrival process and request shapes.

use sim_core::rng::Rng;
use sim_scenario::Scenario;
use sim_core::{Nanos, SECOND};

#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    /// The first attempt's arrival. Latency is measured from here, so a retry does not reset the
    /// clock: from a user's point of view the wait started when they asked.
    pub arrived_at: Nanos,
    pub attempt_at: Nanos,
    pub prompt: u32,
    pub output: u32,
    pub attempts: u32,
    pub deadline: Nanos,
    /// True for the long mode of the mixture. Kept so the report can show that the tail is the long
    /// requests rather than asserting it.
    pub is_long: bool,
    /// Which tenant sent it. Zero when the scenario has no tenancy.
    pub tenant: u32,
}

pub struct Workload {
    next_id: u64,
    arrivals: Rng,
    shapes: Rng,
    /// Its own stream, so enabling tenancy cannot perturb arrivals or shapes: an A/B between a
    /// fair-share policy and none must see byte-identical load.
    tenants: Rng,
    /// Loaded on the first call in trace mode. `new` only sees the seed streams, and a synthetic run
    /// must never touch the file system, so the file cannot be read any earlier.
    trace: Option<Trace>,
}

struct TraceRow {
    t_ns: Nanos,
    prompt: u32,
    output: u32,
    tenant: u32,
}

/// A recorded workload, replayed row by row. Times are offsets from the first row rather than from
/// zero, because the leaf fires its first arrival at the start of the run unconditionally: anchoring
/// the trace there lets a window cut from a longer capture replay without editing its timestamps.
struct Trace {
    rows: Vec<TraceRow>,
    next: usize,
}

impl Trace {
    fn load(path: &str) -> Trace {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("workload = trace but trace_file {path:?} cannot be read: {e}"));
        let mut rows = Vec::new();
        for (n, line) in text.lines().enumerate().skip(1) {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut f = line.split(',').map(str::trim);
            let mut field = |what: &str| {
                f.next()
                    .unwrap_or_else(|| panic!("{path}:{}: missing {what}", n + 1))
                    .parse::<f64>()
                    .unwrap_or_else(|_| panic!("{path}:{}: {what} is not a number", n + 1))
            };
            let t_s = field("t_s");
            let prompt = field("prompt_tokens") as u32;
            let output = field("output_tokens") as u32;
            let tenant = field("tenant") as u32;
            rows.push(TraceRow { t_ns: (t_s * 1e9).round() as Nanos, prompt, output, tenant });
        }
        assert!(!rows.is_empty(), "trace_file {path:?} has no rows after the header");
        let origin = rows[0].t_ns;
        for r in &mut rows {
            r.t_ns -= origin;
        }
        Trace { rows, next: 0 }
    }
}

impl Workload {
    pub fn new(seed_streams: &sim_core::rng::Streams) -> Self {
        Workload {
            next_id: 0,
            arrivals: seed_streams.stream("arrival"),
            shapes: seed_streams.stream("shape"),
            tenants: seed_streams.stream("tenant"),
            trace: None,
        }
    }

    fn trace(&mut self, sc: &Scenario) -> &mut Trace {
        self.trace.get_or_insert_with(|| Trace::load(&sc.trace_file))
    }

    /// Offered rate at a given simulated offset, including any step change. The step is how a
    /// collapse is driven and, more importantly, how recovery is tested after load returns to normal.
    pub fn rate_at(sc: &Scenario, elapsed_s: f64) -> f64 {
        let stepping = sc.load_step_at_s >= 0.0
            && elapsed_s >= sc.load_step_at_s
            && (sc.load_step_until_s < 0.0 || elapsed_s < sc.load_step_until_s);
        if stepping {
            sc.arrival_rps * sc.load_step_factor
        } else {
            sc.arrival_rps
        }
    }

    pub fn next_gap_ns(&mut self, sc: &Scenario, elapsed_s: f64) -> Nanos {
        if sc.workload == "trace" {
            // Rebuilt from the f64 rather than accumulated, so the leaf's 1 ns floor on a zero gap
            // cannot drift later rows off their recorded times. The rounding recovers the exact
            // nanosecond: an f64 carries a run's worth of nanoseconds without loss.
            let elapsed_ns = (elapsed_s * 1e9).round() as Nanos;
            let end_ns = (sc.duration_s * 1e9) as Nanos;
            let t = self.trace(sc);
            return match t.rows.get(t.next) {
                Some(row) => row.t_ns.saturating_sub(elapsed_ns),
                // Past the last row: land after the end of the run, so arrivals stop.
                None => end_ns.saturating_sub(elapsed_ns) + SECOND,
            };
        }
        let rate = Self::rate_at(sc, elapsed_s).max(1e-9);
        let gap = self.arrivals.exponential(1.0 / rate);
        (gap * 1e9) as Nanos
    }

    pub fn make(&mut self, sc: &Scenario, now: Nanos) -> Request {
        self.next_id += 1;
        if sc.workload == "trace" {
            let id = self.next_id;
            let t = self.trace(sc);
            let row = &t.rows[t.next.min(t.rows.len() - 1)];
            t.next += 1;
            return Request {
                id,
                arrived_at: now,
                attempt_at: now,
                prompt: row.prompt,
                output: row.output,
                attempts: 1,
                deadline: now + (sc.client_timeout_s * 1e9) as Nanos,
                // A trace has no mixture label, so the split is by length, at the geometric mean of
                // the two prompt means: the point equidistant from both modes on a log scale.
                is_long: (row.prompt as f64) >= (sc.prompt_mean * sc.long_prompt_mean).sqrt(),
                // Clamped, because the leaf indexes per-tenant state by this and a trace recorded
                // with more tenants than the scenario declares must not crash the run.
                tenant: row.tenant.min(sc.tenants.saturating_sub(1) as u32),
            };
        }
        let long = self.shapes.f64() < sc.long_probability;
        let (p_mean, o_mean) = if long {
            (sc.long_prompt_mean, sc.long_output_mean)
        } else {
            (sc.prompt_mean, sc.output_mean)
        };
        let prompt = self.shapes.lognormal(p_mean, sc.prompt_cv).max(1.0) as u32;
        let output = self.shapes.lognormal(o_mean, sc.output_cv).max(1.0) as u32;
        let tenant = if sc.tenants > 1 {
            let u = self.tenants.f64();
            let mut acc = 0.0;
            let shares = sc.tenant_demand_shares();
            let mut pick = shares.len() - 1;
            for (i, w) in shares.iter().enumerate() {
                acc += w;
                if u < acc {
                    pick = i;
                    break;
                }
            }
            pick as u32
        } else {
            0
        };
        Request {
            id: self.next_id,
            arrived_at: now,
            attempt_at: now,
            prompt,
            output,
            attempts: 1,
            deadline: now + (sc.client_timeout_s * 1e9) as Nanos,
            is_long: long,
            tenant,
        }
    }
}
