//! Scenario text format: reject typos, and lose nothing on a round trip.
//!
//! A scenario file is the entire input to a run, and the module docs are explicit that an unknown key
//! must be an error rather than a silent no-op, because a typo in a scenario is otherwise invisible and
//! the run quietly measures something else. The round trip matters for the same reason from the other
//! side: reports and archived runs are written out through `to_text`, and a field that fails to survive
//! that trip makes a saved scenario un-reproducible.

use lbsim::scenario::Scenario;

/// Every key `parse` accepts, which is also every key `to_text` must emit.
const KEYS: [&str; 55] = [
    "name", "seed", "duration_s", "warmup_s", "replicas", "max_batch", "step_base_ms",
    "step_per_seq_ms", "step_per_kv_ktoken_ms", "kv_capacity_tokens", "prefill_tokens_per_s",
    "step_token_budget", "max_queue", "disable_decode", "preemption", "preemption_victim",
    "dram_capacity_tokens", "swap_gbps", "arrival_rps", "prompt_mean", "prompt_cv", "output_mean",
    "output_cv", "long_probability", "long_prompt_mean", "long_output_mean", "session_turns_mean",
    "session_think_s", "load_step_at_s",
    "load_step_factor", "load_step_until_s", "routing", "p2c_choices", "probe_live",
    "admission", "admission_headroom", "fair_share_burst", "tenants", "tenant_weights", "tenant_demand",
    "telemetry_interval_ms", "telemetry_delay_ms", "client_timeout_s", "max_attempts",
    "retry_budget_fraction", "retry_backoff_s", "ttft_slo_ms", "itl_slo_ms", "e2e_slo_s",
    "sample_interval_ms", "trace_sample_rate", "workload", "trace_file", "spec_draft_tokens",
    "spec_accept_rate",
];

/// A scenario in which no field holds its default value, so a field that silently falls back to the
/// default on the way through `to_text` is caught rather than masked.
fn all_fields_distinct() -> Scenario {
    Scenario {
        name: "round_trip_fixture".into(),
        seed: 987_654_321,
        duration_s: 123.5,
        warmup_s: 7.25,
        replicas: 11,
        max_batch: 13,
        step_base_ms: 9.125,
        step_per_seq_ms: 0.375,
        step_per_kv_ktoken_ms: 0.0225,
        kv_capacity_tokens: 777_000.0,
        prefill_tokens_per_s: 31_250.0,
        step_token_budget: 1536,
        max_queue: 17,
        disable_decode: true,
        preemption: "swap_else_recompute".into(),
        preemption_victim: "largest_kv".into(),
        dram_capacity_tokens: 2_222_000.0,
        swap_gbps: 32.5,
        arrival_rps: 19.5,
        prompt_mean: 1500.5,
        prompt_cv: 1.75,
        output_mean: 275.25,
        output_cv: 2.5,
        long_probability: 0.13,
        long_prompt_mean: 18_000.5,
        long_output_mean: 512.5,
        session_turns_mean: 3.5,
        session_think_s: 6.5,
        load_step_at_s: 21.0,
        load_step_factor: 2.5,
        load_step_until_s: 41.0,
        routing: "least_queue_tokens".into(),
        p2c_choices: 5,
        probe_live: true,
        admission: "accept_all".into(),
        admission_headroom: 0.35,
        fair_share_burst: 3.5,
        tenants: 3,
        tenant_weights: vec![1.0, 2.5, 4.0],
        tenant_demand: vec![4.0, 1.0, 1.0],
        telemetry_interval_ms: 333.5,
        telemetry_delay_ms: 111.25,
        client_timeout_s: 45.5,
        max_attempts: 4,
        retry_budget_fraction: 0.15,
        retry_backoff_s: 1.25,
        ttft_slo_ms: 1500.5,
        itl_slo_ms: 65.5,
        e2e_slo_s: 22.5,
        sample_interval_ms: 125.5,
        trace_sample_rate: 0.35,
        workload: "trace".into(),
        trace_file: "scenarios/traces/sample.csv".into(),
        spec_draft_tokens: 6,
        spec_accept_rate: 0.65,
    }
}

#[test]
fn to_text_then_parse_preserves_every_field() {
    let want = all_fields_distinct();
    let got = Scenario::parse(&want.to_text()).expect("round-tripped text did not parse");

    assert_eq!(got.name, want.name);
    assert_eq!(got.seed, want.seed);
    assert_eq!(got.duration_s, want.duration_s);
    assert_eq!(got.warmup_s, want.warmup_s);
    assert_eq!(got.replicas, want.replicas);
    assert_eq!(got.max_batch, want.max_batch);
    assert_eq!(got.step_base_ms, want.step_base_ms);
    assert_eq!(got.step_per_seq_ms, want.step_per_seq_ms);
    assert_eq!(got.step_per_kv_ktoken_ms, want.step_per_kv_ktoken_ms);
    assert_eq!(got.kv_capacity_tokens, want.kv_capacity_tokens);
    assert_eq!(got.prefill_tokens_per_s, want.prefill_tokens_per_s);
    assert_eq!(got.step_token_budget, want.step_token_budget);
    assert_eq!(got.max_queue, want.max_queue);
    assert_eq!(got.preemption, want.preemption);
    assert_eq!(got.preemption_victim, want.preemption_victim);
    assert_eq!(got.dram_capacity_tokens, want.dram_capacity_tokens);
    assert_eq!(got.swap_gbps, want.swap_gbps);
    assert_eq!(got.arrival_rps, want.arrival_rps);
    assert_eq!(got.prompt_mean, want.prompt_mean);
    assert_eq!(got.prompt_cv, want.prompt_cv);
    assert_eq!(got.output_mean, want.output_mean);
    assert_eq!(got.output_cv, want.output_cv);
    assert_eq!(got.long_probability, want.long_probability);
    assert_eq!(got.long_prompt_mean, want.long_prompt_mean);
    assert_eq!(got.long_output_mean, want.long_output_mean);
    assert_eq!(got.session_turns_mean, want.session_turns_mean);
    assert_eq!(got.session_think_s, want.session_think_s);
    assert_eq!(got.load_step_at_s, want.load_step_at_s);
    assert_eq!(got.load_step_factor, want.load_step_factor);
    assert_eq!(got.load_step_until_s, want.load_step_until_s);
    assert_eq!(got.routing, want.routing);
    assert_eq!(got.p2c_choices, want.p2c_choices);
    assert_eq!(got.probe_live, want.probe_live);
    assert_eq!(got.telemetry_interval_ms, want.telemetry_interval_ms);
    assert_eq!(got.telemetry_delay_ms, want.telemetry_delay_ms);
    assert_eq!(got.client_timeout_s, want.client_timeout_s);
    assert_eq!(got.max_attempts, want.max_attempts);
    assert_eq!(got.retry_budget_fraction, want.retry_budget_fraction);
    assert_eq!(got.retry_backoff_s, want.retry_backoff_s);
    assert_eq!(got.ttft_slo_ms, want.ttft_slo_ms);
    assert_eq!(got.itl_slo_ms, want.itl_slo_ms);
    assert_eq!(got.e2e_slo_s, want.e2e_slo_s);
    assert_eq!(got.sample_interval_ms, want.sample_interval_ms);
    assert_eq!(got.trace_sample_rate, want.trace_sample_rate);
    assert_eq!(got.workload, want.workload);
    assert_eq!(got.trace_file, want.trace_file);
    assert_eq!(got.spec_draft_tokens, want.spec_draft_tokens);
    assert_eq!(got.spec_accept_rate, want.spec_accept_rate);

    // And the trip is idempotent, so an archived scenario re-saved is byte-identical.
    assert_eq!(got.to_text(), want.to_text());
}

/// `to_text` must emit exactly the keys `parse` accepts. If a field is added to `Scenario` and wired
/// into `parse` but forgotten in `to_text`, the round trip above still passes for every *other* field
/// and the loss is invisible; this is the test that notices.
#[test]
fn to_text_emits_every_parseable_key_and_nothing_else() {
    let text = all_fields_distinct().to_text();
    let emitted: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('=').next().unwrap().trim())
        .collect();

    for key in KEYS {
        assert!(emitted.contains(&key), "to_text() does not emit {key}, so it cannot round-trip");
    }
    for key in &emitted {
        assert!(KEYS.contains(key), "to_text() emits {key}, which is not in this test's key list; \
             a field was added to Scenario and this test needs the new key");
    }
    assert_eq!(
        emitted.len(),
        KEYS.len(),
        "to_text() emitted {} lines for {} keys",
        emitted.len(),
        KEYS.len()
    );

    // Every emitted key must actually be understood on the way back in.
    for key in KEYS {
        let one_line = text.lines().find(|l| l.starts_with(key)).unwrap();
        Scenario::parse(one_line)
            .unwrap_or_else(|e| panic!("parse rejected its own output line {one_line:?}: {e}"));
    }
}

/// An unknown key is an error, and the error names it. Silently ignoring `arrival_rate` in a file that
/// meant `arrival_rps` would produce a run at the default 40 rps that looks entirely plausible.
#[test]
fn unknown_keys_are_rejected_and_named() {
    let err = Scenario::parse("replicas = 4\narrival_rate = 12\n")
        .expect_err("an unknown key was accepted");
    assert!(err.contains("arrival_rate"), "error does not name the unknown key: {err}");

    // Every unknown key is reported, not just the first, so one pass fixes a whole file.
    let err = Scenario::parse("qps = 1\nreplicas = 4\nzzz_typo = 2\n")
        .expect_err("unknown keys were accepted");
    assert!(err.contains("qps") && err.contains("zzz_typo"), "not all unknown keys reported: {err}");
}

/// Near-misses of real keys are the ones that matter: they are the typos a person actually makes, and
/// each would otherwise leave a run measuring the default.
#[test]
fn plausible_typos_of_real_keys_are_rejected() {
    for typo in [
        "replica = 8",
        "arrival_rps_ = 8",
        "Replicas = 8",
        "max_batch_size = 8",
        "seed_ = 8",
        "ttft_slo = 2000",
        "duration = 30",
    ] {
        assert!(
            Scenario::parse(typo).is_err(),
            "{typo:?} was silently accepted; a run with this line would measure the defaults"
        );
    }
}

#[test]
fn comments_blank_lines_and_whitespace_are_tolerated() {
    let text = "\n\
        # a leading comment\n\
        \n\
           replicas   =   6   # trailing comment\n\
        \t routing\t=\tp2c\n\
        # arrival_rps = 999 (commented out entirely)\n\
        seed = 5\n\
        \n";
    let s = Scenario::parse(text).expect("comments or whitespace broke the parse");
    assert_eq!(s.replicas, 6);
    assert_eq!(s.routing, "p2c");
    assert_eq!(s.seed, 5);
    assert_eq!(
        s.arrival_rps,
        Scenario::default().arrival_rps,
        "a commented-out key took effect"
    );
}

#[test]
fn empty_text_yields_the_defaults() {
    let s = Scenario::parse("").expect("empty scenario should be the default scenario");
    assert_eq!(s.to_text(), Scenario::default().to_text());

    let s = Scenario::parse("# nothing but comments\n\n").unwrap();
    assert_eq!(s.to_text(), Scenario::default().to_text());
}

/// A line with no `=` is a structural error and must report where it is, since scenario files are
/// hand-written.
#[test]
fn a_line_without_a_separator_is_rejected_with_its_line_number() {
    let err = Scenario::parse("replicas = 4\nthis is not a setting\n")
        .expect_err("a malformed line was accepted");
    assert!(err.contains("line 2"), "error does not locate the bad line: {err}");
}

/// A later assignment of the same key wins, so the format has one unambiguous meaning for a file that
/// sets something twice.
#[test]
fn a_repeated_key_takes_its_last_value() {
    let s = Scenario::parse("replicas = 4\nreplicas = 9\n").unwrap();
    assert_eq!(s.replicas, 9);
}

/// `rated_rps` is the denominator every load in this suite is expressed in, so its shape has to be
/// right: proportional to the fleet, and falling as requests get more expensive.
#[test]
fn rated_capacity_scales_with_the_fleet_and_the_cost_of_a_request() {
    let mut s = Scenario::default();
    s.replicas = 8;
    let base = s.rated_rps();
    assert!(base > 0.0 && base.is_finite(), "rated_rps is {base}");

    s.replicas = 16;
    assert!(
        (s.rated_rps() / base - 2.0).abs() < 1e-9,
        "doubling the fleet did not double rated capacity: {} vs {base}",
        s.rated_rps()
    );

    s.replicas = 8;
    s.output_mean *= 4.0;
    assert!(s.rated_rps() < base, "quadrupling output length did not reduce rated capacity");

    s.output_mean = Scenario::default().output_mean;
    s.prompt_mean *= 4.0;
    assert!(s.rated_rps() < base, "quadrupling prompt length did not reduce rated capacity");
}
