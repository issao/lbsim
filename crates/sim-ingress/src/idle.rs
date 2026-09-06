//! Idle shutdown: the decision half of docs/execution-plan.md section 3.5 item 2.
//!
//! "With no live subscription lease and no queued work for IDLE_SHUTDOWN_SECONDS, checkpoint the run
//! to the bucket and stop advancing so the instance can be reaped. This is the single most important
//! cost control." Cloud Run bills while the container is doing anything, and an Ingress that keeps
//! advancing a simulation nobody is watching is the one way this project can spend money by accident.
//!
//! This module only *decides*. Checkpointing the run and stopping the engine are the server's job
//! and depend on the engine snapshot work happening elsewhere; the guard has no handle on either,
//! which is what keeps it trivially testable and free of simulation state. The server polls it,
//! passing the wall clock, the live-lease count from [`crate::lease::LeaseRegistry::live`] and its
//! own queue depth, and acts on [`IdleDecision::Shutdown`]. WIRE.md ("Leases and idle shutdown")
//! fixes what the client sees afterwards: `GetRun` reports `STATE_PAUSED` with `error` empty, and
//! reopening a subscription resumes the run, which is when the server calls [`IdleGuard::resume`].
//! The guard has no notion of a run, so the server may hold one per run (WIRE.md's wording) or one
//! per instance (the execution plan's); the rule is the same either way.
//!
//! No clock inside: `now` is an argument for the same reason as in [`crate::lease`]. The one place
//! that touches `std::time` is [`wall_now_ns`], a helper for the server that the guard never calls.

pub const DEFAULT_IDLE_SHUTDOWN_SECONDS: u64 = 300;

/// Reads `IDLE_SHUTDOWN_SECONDS` from the environment. Default 300. A value that does not parse
/// falls back to the default rather than to zero, because zero would reap an instance instantly and
/// a typo in a deploy flag should degrade to the documented behaviour, not to an outage.
pub fn idle_threshold_ns_from_env() -> u64 {
    parse_threshold(std::env::var("IDLE_SHUTDOWN_SECONDS").ok().as_deref())
}

/// The parse behind [`idle_threshold_ns_from_env`], separated so it can be tested without touching
/// the process environment, which is shared with every other test in the binary.
///
/// Saturating so a value like `99999999999999` becomes "never" rather than wrapping into "now".
pub(crate) fn parse_threshold(raw: Option<&str>) -> u64 {
    let seconds = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_SHUTDOWN_SECONDS);
    seconds.saturating_mul(1_000_000_000)
}

/// Wall clock for the server's polling loop. Deliberately the only `std::time` use in the crate's
/// lease and idle machinery, and deliberately outside the guard and the registry: they take this
/// value as an argument so tests can run them at any instant.
pub fn wall_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleDecision {
    /// A live lease or queued work: the idle clock is reset.
    Busy,
    /// Nothing to do, for `ns` so far. Informational, so the server can log or expose it.
    IdleFor { ns: u64 },
    /// Continuously idle for at least the threshold. Returned once per idle stretch.
    Shutdown,
}

#[derive(Debug)]
pub struct IdleGuard {
    threshold_ns: u64,
    /// Start of the current idle stretch. `None` while busy.
    idle_since: Option<u64>,
    /// Whether `Shutdown` has already been returned for the current idle stretch. The server acts
    /// on it once; repeating it every poll would re-trigger the checkpoint.
    fired: bool,
}

impl IdleGuard {
    pub fn new(threshold_ns: u64) -> Self {
        Self { threshold_ns, idle_since: None, fired: false }
    }

    pub fn threshold_ns(&self) -> u64 {
        self.threshold_ns
    }

    /// Called by the server on every poll. The exact rule:
    ///
    /// 1. If `live_leases > 0` or `queued_work > 0`: `Busy`. The idle stretch ends; `idle_since`
    ///    is cleared and `fired` is cleared, so the next idle stretch is measured afresh and can
    ///    fire again. No `resume()` is needed if the instance became busy before it was reaped.
    /// 2. Otherwise the instance is idle. If this poll starts the stretch, `idle_since = now`.
    /// 3. If `now - idle_since >= threshold` and `Shutdown` has not been returned for this stretch:
    ///    return `Shutdown` and remember that it fired.
    /// 4. Otherwise `IdleFor { ns: now - idle_since }`, including after `Shutdown` has fired: the
    ///    server has already been told once, and a checkpoint that is still in flight must not be
    ///    started twice. `resume()` re-arms it.
    ///
    /// `now` going backwards (a clock step) is treated as no time having passed rather than as a
    /// wrap: an underflow would look like an enormous idle stretch and reap a live instance.
    pub fn observe(&mut self, now_wall_ns: u64, live_leases: usize, queued_work: usize) -> IdleDecision {
        if live_leases > 0 || queued_work > 0 {
            self.idle_since = None;
            self.fired = false;
            return IdleDecision::Busy;
        }
        let since = *self.idle_since.get_or_insert(now_wall_ns);
        let idle_ns = now_wall_ns.saturating_sub(since);
        if idle_ns >= self.threshold_ns && !self.fired {
            self.fired = true;
            return IdleDecision::Shutdown;
        }
        IdleDecision::IdleFor { ns: idle_ns }
    }

    /// The run was resumed after a shutdown decision, typically because a lease opened while the
    /// checkpoint was in flight or after the instance was revived. Clears `fired` and `idle_since`
    /// so the run can be reaped again later, measured from the next idle poll.
    pub fn resume(&mut self) {
        self.idle_since = None;
        self.fired = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn idle_guard_fires_once_after_the_threshold_with_no_leases_and_no_work() {
        let mut g = IdleGuard::new(300 * S);
        assert_eq!(g.observe(1000 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
        assert_eq!(g.observe(1100 * S, 0, 0), IdleDecision::IdleFor { ns: 100 * S });
        assert_eq!(g.observe(1300 * S - 1, 0, 0), IdleDecision::IdleFor { ns: 300 * S - 1 });
        assert_eq!(g.observe(1300 * S, 0, 0), IdleDecision::Shutdown, "fires at exactly the threshold");
        assert_eq!(g.observe(1301 * S, 0, 0), IdleDecision::IdleFor { ns: 301 * S });
        assert_eq!(g.observe(9999 * S, 0, 0), IdleDecision::IdleFor { ns: 8999 * S }, "never twice per stretch");
    }

    #[test]
    fn idle_guard_resets_when_a_lease_opens() {
        let mut g = IdleGuard::new(300 * S);
        g.observe(1000 * S, 0, 0);
        assert_eq!(g.observe(1200 * S, 1, 0), IdleDecision::Busy);
        // The clock restarts from the first idle poll after the lease is gone.
        assert_eq!(g.observe(1250 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
        assert_eq!(g.observe(1300 * S, 0, 0), IdleDecision::IdleFor { ns: 50 * S });
        assert_eq!(g.observe(1550 * S, 0, 0), IdleDecision::Shutdown);
        // A lease after shutdown also re-arms, without an explicit resume().
        assert_eq!(g.observe(1600 * S, 1, 0), IdleDecision::Busy);
        assert_eq!(g.observe(1700 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
        assert_eq!(g.observe(2000 * S, 0, 0), IdleDecision::Shutdown);
    }

    #[test]
    fn idle_guard_treats_queued_work_as_busy() {
        let mut g = IdleGuard::new(300 * S);
        assert_eq!(g.observe(1000 * S, 0, 3), IdleDecision::Busy);
        assert_eq!(g.observe(1400 * S, 0, 1), IdleDecision::Busy, "work with no leases is still work");
        assert_eq!(g.observe(1500 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
        assert_eq!(g.observe(1799 * S, 0, 0), IdleDecision::IdleFor { ns: 299 * S });
        assert_eq!(g.observe(1800 * S, 0, 0), IdleDecision::Shutdown);
    }

    #[test]
    fn idle_guard_resume_allows_a_second_shutdown() {
        let mut g = IdleGuard::new(300 * S);
        g.observe(1000 * S, 0, 0);
        assert_eq!(g.observe(1300 * S, 0, 0), IdleDecision::Shutdown);
        assert_eq!(g.observe(1400 * S, 0, 0), IdleDecision::IdleFor { ns: 400 * S });
        g.resume();
        // Measured afresh from the next idle poll, not from the old idle_since.
        assert_eq!(g.observe(1400 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
        assert_eq!(g.observe(1699 * S, 0, 0), IdleDecision::IdleFor { ns: 299 * S });
        assert_eq!(g.observe(1700 * S, 0, 0), IdleDecision::Shutdown);
        assert_eq!(g.observe(1800 * S, 0, 0), IdleDecision::IdleFor { ns: 400 * S });
    }

    #[test]
    fn idle_guard_zero_threshold_fires_on_the_first_idle_poll() {
        let mut g = IdleGuard::new(0);
        assert_eq!(g.observe(1000 * S, 0, 0), IdleDecision::Shutdown);
        assert_eq!(g.observe(1001 * S, 0, 0), IdleDecision::IdleFor { ns: S });
    }

    #[test]
    fn idle_guard_tolerates_a_clock_step_backwards() {
        let mut g = IdleGuard::new(300 * S);
        g.observe(1000 * S, 0, 0);
        assert_eq!(g.observe(900 * S, 0, 0), IdleDecision::IdleFor { ns: 0 });
    }

    #[test]
    fn threshold_defaults_to_300_seconds_and_survives_garbage() {
        assert_eq!(parse_threshold(None), 300 * S);
        assert_eq!(parse_threshold(Some("")), 300 * S);
        assert_eq!(parse_threshold(Some("   ")), 300 * S);
        assert_eq!(parse_threshold(Some("abc")), 300 * S);
        assert_eq!(parse_threshold(Some("-5")), 300 * S);
        assert_eq!(parse_threshold(Some("1.5")), 300 * S);
        assert_eq!(parse_threshold(Some("60")), 60 * S);
        assert_eq!(parse_threshold(Some(" 60 ")), 60 * S);
        assert_eq!(parse_threshold(Some("0")), 0, "an explicit zero is honoured; only garbage falls back");
        assert_eq!(parse_threshold(Some("99999999999999")), u64::MAX, "saturates to never, not to now");
        assert_eq!(DEFAULT_IDLE_SHUTDOWN_SECONDS, 300);
    }
}
