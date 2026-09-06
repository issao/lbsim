//! Arrival process and request shapes.

use sim_core::rng::Rng;
use sim_scenario::Scenario;
use sim_core::Nanos;

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
}

impl Workload {
    pub fn new(seed_streams: &sim_core::rng::Streams) -> Self {
        Workload {
            next_id: 0,
            arrivals: seed_streams.stream("arrival"),
            shapes: seed_streams.stream("shape"),
            tenants: seed_streams.stream("tenant"),
        }
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
        let rate = Self::rate_at(sc, elapsed_s).max(1e-9);
        let gap = self.arrivals.exponential(1.0 / rate);
        (gap * 1e9) as Nanos
    }

    pub fn make(&mut self, sc: &Scenario, now: Nanos) -> Request {
        self.next_id += 1;
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
            let shares = sc.tenant_shares();
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
