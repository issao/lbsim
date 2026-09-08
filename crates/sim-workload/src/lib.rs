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
    /// SLO class, an index into `sim_scenario::SLO_CLASSES` plus one. Zero when classes are off.
    pub class: u8,
    /// The shared part of `prompt`, as a node of the `PrefixTree`; zero when the scenario has no
    /// prefix model. The first `prefix_tokens` of the prompt are that node's path from the root,
    /// and the remainder is this request's own suffix.
    pub prefix_node: u64,
    pub prefix_tokens: u32,
}

/// Shared prefixes as a tree of segments, `docs/ARCHITECTURE.md` section 7.3: a request carries one
/// node and its unshared suffix length, and how much of a prompt is resident somewhere is a walk up
/// this tree, never a comparison of content. Sharing is generated here rather than discovered: turn
/// N of a session extends turn N-1's node, a shared system prompt is a common root, a fork creates
/// siblings under a shared parent. Node 0 is "none"; roots are `1..=prefix_roots`.
#[derive(Clone, Debug, Default)]
pub struct PrefixTree {
    parent: Vec<u64>,
    tokens: Vec<u32>,
}

impl PrefixTree {
    /// Roots created up front, their lengths drawn once from `stream`, so the topology is fixed by
    /// the scenario and seed before the first arrival. Empty when the scenario has no prefix model.
    pub fn new(sc: &Scenario, stream: &mut Rng) -> PrefixTree {
        let mut t = PrefixTree { parent: vec![0], tokens: vec![0] };
        for _ in 0..sc.prefix_roots {
            let tokens = stream.lognormal(sc.prefix_root_tokens, 0.5).max(1.0) as u32;
            t.child(0, tokens);
        }
        t
    }

    /// No nodes at all, not even the "none" node; every lookup answers zero. `const` so a replica
    /// stepped without a prefix model can name one without allocating.
    pub const fn empty() -> PrefixTree {
        PrefixTree { parent: Vec::new(), tokens: Vec::new() }
    }

    pub fn child(&mut self, parent: u64, tokens: u32) -> u64 {
        self.parent.push(parent);
        self.tokens.push(tokens);
        (self.parent.len() - 1) as u64
    }

    pub fn parent(&self, node: u64) -> u64 {
        self.parent.get(node as usize).copied().unwrap_or(0)
    }

    pub fn tokens(&self, node: u64) -> u32 {
        self.tokens.get(node as usize).copied().unwrap_or(0)
    }

    /// Tokens along the chain from the root to `node` inclusive: the length of the prefix it names.
    pub fn path_tokens(&self, node: u64) -> u32 {
        let mut n = node;
        let mut sum = 0u32;
        while n != 0 {
            sum = sum.saturating_add(self.tokens(n));
            n = self.parent(n);
        }
        sum
    }

    /// Nodes so far, the "none" node included.
    pub fn nodes(&self) -> usize {
        self.parent.len()
    }
}

pub struct Workload {
    next_id: u64,
    arrivals: Rng,
    shapes: Rng,
    /// Its own stream, so enabling tenancy cannot perturb arrivals or shapes: an A/B between a
    /// fair-share policy and none must see byte-identical load.
    tenants: Rng,
    /// Same reason as `tenants`: turning classes on relabels requests and nothing else.
    classes: Rng,
    /// Root choice for the prefix model; its own stream so `prefix_roots = 0` draws nothing.
    prefix: Rng,
    /// Zipf CDF over the roots, built on first use because it depends on the scenario.
    zipf: Option<Vec<f64>>,
    /// `Scenario::slo_class_shares()` re-parses text, and that is too much per arrival.
    class_shares: Option<Vec<(u8, f64)>>,
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
        // Sorted first: a capture is not guaranteed to arrive in time order, and `origin` must be the
        // earliest row or a later subtraction underflows (Nanos is u64). Stable, so rows that tie on a
        // timestamp keep the file's order.
        rows.sort_by_key(|r| r.t_ns);
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
            classes: seed_streams.stream("class"),
            prefix: seed_streams.stream("prefix"),
            zipf: None,
            class_shares: None,
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

    /// Offered rate over a closing sample window, for the dashboard's offered-load curve. Synthetic
    /// mode has no window to count and stays exactly `rate_at` at the window's end, so a synthetic
    /// run's fingerprint cannot move. Trace mode has no rate to sample at all: it counts the rows
    /// whose recorded arrival (elapsed nanos from the run's start, same base as `next_gap_ns` uses)
    /// falls in `[window_start_ns, window_end_ns)` and divides by the window's width, which is what
    /// the CSV itself says the load was.
    pub fn offered_rps(&self, sc: &Scenario, window_start_ns: Nanos, window_end_ns: Nanos) -> f64 {
        if sc.workload != "trace" {
            return Self::rate_at(sc, window_end_ns as f64 / 1e9);
        }
        let trace = self
            .trace
            .as_ref()
            .expect("offered_rps in trace mode is sampled only after an arrival has loaded the trace");
        let count = trace.rows.iter().filter(|r| r.t_ns >= window_start_ns && r.t_ns < window_end_ns).count();
        let width_s = (window_end_ns - window_start_ns) as f64 / 1e9;
        count as f64 / width_s
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

    /// Only touches the class stream when classes are on, so every classes-off run is byte-identical
    /// to what it was before classes existed.
    fn draw_class(&mut self, sc: &Scenario) -> u8 {
        if sc.slo_classes.is_empty() {
            return 0;
        }
        let shares = self.class_shares.get_or_insert_with(|| sc.slo_class_shares());
        let Some(last) = shares.last() else { return 0 };
        let u = self.classes.f64();
        let mut acc = 0.0;
        for &(c, w) in shares.iter() {
            acc += w;
            if u < acc {
                return c;
            }
        }
        last.0
    }

    /// Which root a fresh arrival starts from: Zipf(`prefix_zipf_s`) over `1..=prefix_roots`, the
    /// shape `docs/calibration.md` section 9.1 guesses for system-prompt popularity. Zero when the
    /// scenario has no prefix model, and then nothing is drawn.
    fn draw_root(&mut self, sc: &Scenario) -> u64 {
        if sc.prefix_roots == 0 {
            return 0;
        }
        let cdf = self.zipf.get_or_insert_with(|| {
            let n = sc.prefix_roots as usize;
            let mut acc = 0.0;
            let mut w: Vec<f64> =
                (1..=n).map(|k| (k as f64).powf(-sc.prefix_zipf_s)).collect();
            let total: f64 = w.iter().sum();
            for x in w.iter_mut() {
                acc += *x / total;
                *x = acc;
            }
            w
        });
        let u = self.prefix.f64();
        let k = cdf.iter().position(|&c| u < c).unwrap_or(cdf.len() - 1);
        (k + 1) as u64
    }

    pub fn make(&mut self, sc: &Scenario, now: Nanos, tree: &PrefixTree) -> Request {
        self.next_id += 1;
        let class = self.draw_class(sc);
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
                class,
                // A trace records lengths, not sharing; the prefix model is synthetic only.
                prefix_node: 0,
                prefix_tokens: 0,
            };
        }
        let long = self.shapes.f64() < sc.long_probability;
        let (p_mean, o_mean) = if long {
            (sc.long_prompt_mean, sc.long_output_mean)
        } else {
            (sc.prompt_mean, sc.output_mean)
        };
        let mut prompt = self.shapes.lognormal(p_mean, sc.prompt_cv).max(1.0) as u32;
        let output = self.shapes.lognormal(o_mean, sc.output_cv).max(1.0) as u32;
        let prefix_node = self.draw_root(sc);
        let prefix_tokens = tree.path_tokens(prefix_node);
        // The system prompt is part of the prompt, and a request always has at least one token of
        // its own after it. With no prefix model this is `max(prompt, 1)`, a no-op.
        prompt = prompt.max(prefix_tokens.saturating_add(1));
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
            class,
            prefix_node,
            prefix_tokens,
        }
    }
}
