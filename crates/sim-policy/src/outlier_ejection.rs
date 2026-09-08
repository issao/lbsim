//! lbsim-policy: health names=outlier,outlier_ejection
//! Eject a replica whose step time is an outlier against the fleet, on the delayed view alone.
//!
//! The mechanism: at every assessment the fleet median of `last_step_ns` is taken over replicas that
//! have stepped at all and are not currently ejected, and a replica whose step time exceeds
//! `ejection_ratio` times that median on `ejection_views` consecutive views is ejected for
//! `ejection_cooldown_s`, then re-admitted and judged afresh. Consecutive *views*, not consecutive
//! calls: the engine assesses on every telemetry delivery, of which there are `replicas` per interval,
//! and a strike is only counted when the replica's own sample has changed, so `ejection_views = 3` at a
//! 1 s interval means three seconds of evidence rather than three deliveries from other replicas.
//! The cooldown is why a single slow step is not a verdict, and the view count is why a single slow
//! sample is not one either. A crash announced through `view.ejected` stays ejected regardless.
//!
//! Cost. This runs once per telemetry delivery, never per request, and the median is O(N) in the
//! fleet: a select over one value per replica. At 10k replicas and a 1 s interval that is 10k
//! medians a second of a 10k-element select, which is why the median is taken over at most
//! `MEDIAN_SAMPLE` views when the fleet is larger, chosen by a fixed stride from index zero so the
//! sample is deterministic and the same on every call. The comparison itself stays over every
//! replica, so the bound is on the reference, not on who can be ejected.

use crate::{HealthPolicy, ReplicaView};
use sim_core::Nanos;
use sim_scenario::Scenario;

/// The largest fleet whose every replica contributes to the median.
const MEDIAN_SAMPLE: usize = 256;

pub struct OutlierEjection {
    ratio: f64,
    views_needed: u32,
    cooldown: Nanos,
    /// Per replica: consecutive outlying views so far, the `sampled_at` of the last view judged, and
    /// when an ejection ends (zero while not ejected). Sized on first use.
    strikes: Vec<u32>,
    judged_at: Vec<Nanos>,
    ejected_until: Vec<Nanos>,
}

pub fn make(sc: &Scenario) -> Box<dyn HealthPolicy> {
    Box::new(OutlierEjection {
        ratio: sc.ejection_ratio,
        views_needed: sc.ejection_views.max(1),
        cooldown: (sc.ejection_cooldown_s * 1e9) as Nanos,
        strikes: Vec::new(),
        judged_at: Vec::new(),
        ejected_until: Vec::new(),
    })
}

impl OutlierEjection {
    fn ensure_sized(&mut self, n: usize) {
        if self.strikes.len() != n {
            self.strikes = vec![0; n];
            self.judged_at = vec![0; n];
            self.ejected_until = vec![0; n];
        }
    }

    /// Median step time of the replicas that can serve as a reference: stepped at least once, not
    /// crashed, not currently ejected. Zero when there is no reference, which ejects nobody.
    fn reference_median(&self, now: Nanos, views: &[ReplicaView]) -> Nanos {
        let stride = views.len().div_ceil(MEDIAN_SAMPLE).max(1);
        let mut v: Vec<Nanos> = views
            .iter()
            .enumerate()
            .filter(|(i, _)| i % stride == 0)
            .filter(|(i, v)| v.last_step_ns > 0 && !v.ejected && self.ejected_until[*i] <= now)
            .map(|(_, v)| v.last_step_ns)
            .collect();
        if v.is_empty() {
            return 0;
        }
        let mid = v.len() / 2;
        *v.select_nth_unstable(mid).1
    }
}

impl HealthPolicy for OutlierEjection {
    fn label(&self) -> String {
        format!("outlier(ratio={}, views={}, cooldown={}s)", self.ratio, self.views_needed, self.cooldown as f64 / 1e9)
    }

    fn assess(&mut self, now: Nanos, views: &[ReplicaView]) -> Vec<bool> {
        self.ensure_sized(views.len());
        let median = self.reference_median(now, views);
        let threshold = self.ratio * median as f64;
        let mut out = Vec::with_capacity(views.len());
        for (i, v) in views.iter().enumerate() {
            if v.ejected {
                // Announced, so no inference is needed; forget any evidence, a crash is not a strike.
                self.strikes[i] = 0;
                out.push(true);
                continue;
            }
            if self.ejected_until[i] > now {
                out.push(true);
                continue;
            }
            if self.ejected_until[i] != 0 {
                // The cooldown ended: back in the rotation with a clean record, judged afresh from here.
                self.ejected_until[i] = 0;
                self.strikes[i] = 0;
                self.judged_at[i] = v.sampled_at;
                out.push(false);
                continue;
            }
            if v.sampled_at != self.judged_at[i] {
                self.judged_at[i] = v.sampled_at;
                if median > 0 && v.last_step_ns as f64 > threshold {
                    self.strikes[i] += 1;
                } else {
                    self.strikes[i] = 0;
                }
            }
            if self.strikes[i] >= self.views_needed {
                self.strikes[i] = 0;
                self.ejected_until[i] = now + self.cooldown.max(1);
                out.push(true);
            } else {
                out.push(false);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Nanos = 1_000_000_000;

    fn policy(ratio: f64, views: u32, cooldown_s: f64) -> OutlierEjection {
        let mut sc = Scenario::default();
        sc.ejection_ratio = ratio;
        sc.ejection_views = views;
        sc.ejection_cooldown_s = cooldown_s;
        OutlierEjection {
            ratio: sc.ejection_ratio,
            views_needed: sc.ejection_views,
            cooldown: (sc.ejection_cooldown_s * 1e9) as Nanos,
            strikes: Vec::new(),
            judged_at: Vec::new(),
            ejected_until: Vec::new(),
        }
    }

    /// Eight replicas at 10 ms steps, the last at `factor` times that, all sampled at `t`.
    fn fleet(factor: f64, t: Nanos) -> Vec<ReplicaView> {
        (0..8)
            .map(|i| ReplicaView {
                sampled_at: t,
                last_step_ns: if i == 7 { (10_000_000.0 * factor) as Nanos } else { 10_000_000 },
                ..ReplicaView::default()
            })
            .collect()
    }

    #[test]
    fn an_outlier_at_3_3x_is_ejected_after_exactly_the_configured_views() {
        let mut p = policy(3.0, 3, 30.0);
        for k in 1..=2 {
            let out = p.assess(k * S, &fleet(3.3, k * S));
            assert!(out.iter().all(|e| !e), "ejected after {k} views, wanted 3");
        }
        let out = p.assess(3 * S, &fleet(3.3, 3 * S));
        assert!(out[7] && out[..7].iter().all(|e| !e), "{out:?}");
    }

    #[test]
    fn a_repeated_delivery_of_the_same_view_is_one_view() {
        let mut p = policy(3.0, 3, 30.0);
        // Ten deliveries between two samples: other replicas' telemetry landing, not new evidence.
        for _ in 0..10 {
            assert!(!p.assess(S, &fleet(3.3, S))[7]);
        }
        assert!(!p.assess(2 * S, &fleet(3.3, 2 * S))[7]);
        assert!(p.assess(3 * S, &fleet(3.3, 3 * S))[7]);
    }

    #[test]
    fn an_outlier_at_2_9x_is_never_ejected() {
        let mut p = policy(3.0, 3, 30.0);
        for k in 1..=20 {
            assert!(p.assess(k * S, &fleet(2.9, k * S)).iter().all(|e| !e), "ejected at view {k}");
        }
    }

    #[test]
    fn an_ejection_expires_after_the_cooldown_and_is_reassessed() {
        let mut p = policy(3.0, 3, 30.0);
        for k in 1..=3 {
            p.assess(k * S, &fleet(3.3, k * S));
        }
        assert!(p.assess(4 * S, &fleet(3.3, 4 * S))[7]);
        assert!(p.assess(32 * S, &fleet(3.3, 32 * S))[7], "still within the cooldown");
        // Recovered by the time the cooldown ends: it stays admitted.
        assert!(!p.assess(34 * S, &fleet(1.0, 34 * S))[7], "not re-admitted after the cooldown");
        for k in 35..=40 {
            assert!(!p.assess(k * S, &fleet(1.0, k * S))[7]);
        }
        // Still slow: it is ejected again, and only after the full count of fresh views.
        let mut p = policy(3.0, 3, 30.0);
        for k in 1..=3 {
            p.assess(k * S, &fleet(3.3, k * S));
        }
        assert!(!p.assess(34 * S, &fleet(3.3, 34 * S))[7]);
        assert!(!p.assess(35 * S, &fleet(3.3, 35 * S))[7]);
        assert!(!p.assess(36 * S, &fleet(3.3, 36 * S))[7]);
        assert!(p.assess(37 * S, &fleet(3.3, 37 * S))[7]);
    }

    #[test]
    fn a_crash_stays_ejected_whatever_its_step_time() {
        let mut p = policy(3.0, 3, 30.0);
        let mut views = fleet(1.0, S);
        views[2].ejected = true;
        for k in 1..=5 {
            views.iter_mut().for_each(|v| v.sampled_at = k * S);
            let out = p.assess(k * S, &views);
            assert!(out[2], "crash forgotten at view {k}");
            assert_eq!(out.iter().filter(|e| **e).count(), 1);
        }
    }

    #[test]
    fn a_large_fleet_samples_the_median_and_still_ejects_the_outlier() {
        let mut p = policy(3.0, 1, 30.0);
        let mut views: Vec<ReplicaView> = (0..4096)
            .map(|_| ReplicaView { sampled_at: S, last_step_ns: 10_000_000, ..ReplicaView::default() })
            .collect();
        // Index 4095 is not on the stride, so it never feeds the median and is still judged.
        views[4095].last_step_ns = 40_000_000;
        let out = p.assess(S, &views);
        assert_eq!(out.iter().filter(|e| **e).count(), 1);
        assert!(out[4095]);
    }

    #[test]
    fn nothing_is_ejected_before_anyone_has_stepped() {
        let mut p = policy(3.0, 1, 30.0);
        let views = vec![ReplicaView { sampled_at: S, ..ReplicaView::default() }; 4];
        assert!(p.assess(S, &views).iter().all(|e| !e));
    }
}
