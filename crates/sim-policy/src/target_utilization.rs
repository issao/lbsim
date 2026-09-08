//! lbsim-policy: autoscaling names=target_utilization,target
//! Hold the fleet's mean utilization at a target, the controller every real autoscaler descends from.
//!
//! Utilization is the mean over the replicas the view shows as serving of `running / max_batch`: the
//! batch is a replica's concurrency, so a full batch is a full replica. The decision is proportional,
//! `ceil(serving * utilization / target)` replicas, taken only past a dead band: up when utilization
//! exceeds the target, down only when it falls below `DOWN_RATIO` of it, so a fleet sitting at its
//! target is left alone rather than nudged every interval. Two more guards make it a controller
//! rather than a reflex. A decision moves at most `autoscale_step` replicas, in either direction:
//! bounded up because a real fleet cannot turn up hundreds of machines at once, bounded down because a
//! single stale view must not halve the fleet. And no scale-down within `autoscale_cooldown_s` of a
//! scale-up, because the replicas just turned up are still warming and the utilization they will
//! absorb is not in the view yet; without that the controller drains the fleet it is in the middle of
//! growing.
//!
//! What the view does not show is the lag this policy pays for: a queue that formed since the last
//! telemetry, and a replica whose first sample has not landed. Both are the engine's, not the policy's.

use crate::autoscaling::{AutoscalingPolicy, FleetView};
use sim_core::Nanos;
use sim_scenario::Scenario;

/// Scale up above `target`; scale down only below `target * DOWN_RATIO`. The hysteresis that keeps a
/// fleet at its target from oscillating between two sizes.
const UP_RATIO: f64 = 1.0;
const DOWN_RATIO: f64 = 0.5;

pub struct TargetUtilization {
    target: f64,
    step: usize,
    cooldown: Nanos,
    max_batch: usize,
    /// When the last scale-up was decided; zero before any.
    last_up_at: Nanos,
}

pub fn make(sc: &Scenario) -> Box<dyn AutoscalingPolicy> {
    Box::new(TargetUtilization {
        target: sc.autoscale_target,
        step: sc.autoscale_step.max(1),
        cooldown: (sc.autoscale_cooldown_s * 1e9) as Nanos,
        max_batch: sc.max_batch.max(1),
        last_up_at: 0,
    })
}

impl AutoscalingPolicy for TargetUtilization {
    fn label(&self) -> String {
        format!(
            "target_utilization(target={}, step={}, cooldown={}s)",
            self.target,
            self.step,
            self.cooldown as f64 / 1e9
        )
    }

    fn desired(&mut self, v: &FleetView) -> usize {
        let current = v.ready + v.warming;
        let serving: Vec<&crate::ReplicaView> = v.views.iter().filter(|x| !x.ejected).collect();
        if serving.is_empty() || self.target <= 0.0 {
            return current.clamp(v.min, v.max);
        }
        let running: u64 = serving.iter().map(|x| u64::from(x.running)).sum();
        let util = running as f64 / (serving.len() * self.max_batch) as f64;
        let need = (serving.len() as f64 * util / self.target).ceil() as usize;
        let mut want = current;
        if util > self.target * UP_RATIO && need > current {
            want = need.min(current + self.step);
            self.last_up_at = v.now.max(1);
        } else if util < self.target * DOWN_RATIO && need < current {
            let cooling = self.last_up_at != 0 && v.now < self.last_up_at + self.cooldown;
            if !cooling {
                want = need.max(current.saturating_sub(self.step));
            }
        }
        want.clamp(v.min, v.max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReplicaView;

    const S: Nanos = 1_000_000_000;

    fn policy(target: f64, step: usize, cooldown_s: f64) -> TargetUtilization {
        let mut sc = Scenario::default();
        sc.autoscale_target = target;
        sc.autoscale_step = step;
        sc.autoscale_cooldown_s = cooldown_s;
        sc.max_batch = 10;
        TargetUtilization {
            target: sc.autoscale_target,
            step: sc.autoscale_step,
            cooldown: (sc.autoscale_cooldown_s * 1e9) as Nanos,
            max_batch: sc.max_batch,
            last_up_at: 0,
        }
    }

    /// `ready` serving replicas each running `running` of a batch of 10, plus `absent` ejected slots.
    fn fleet(ready: usize, running: u32, absent: usize) -> Vec<ReplicaView> {
        let mut v: Vec<ReplicaView> =
            (0..ready).map(|_| ReplicaView { running, ..ReplicaView::default() }).collect();
        v.extend((0..absent).map(|_| ReplicaView { ejected: true, ..ReplicaView::default() }));
        v
    }

    fn view<'a>(now: Nanos, views: &'a [ReplicaView], ready: usize, warming: usize) -> FleetView<'a> {
        FleetView { now, ready, warming, draining: 0, max: 16, min: 1, views }
    }

    #[test]
    fn inside_the_dead_band_nothing_moves() {
        let mut p = policy(0.7, 8, 30.0);
        for running in [4, 5, 6, 7] {
            let views = fleet(4, running, 4);
            assert_eq!(p.desired(&view(S, &views, 4, 0)), 4, "running {running}");
        }
    }

    #[test]
    fn a_full_batch_scales_up_proportionally_and_no_more_than_the_step() {
        let mut p = policy(0.7, 8, 30.0);
        // 4 replicas at 1.0 want ceil(4 / 0.7) = 6.
        let views = fleet(4, 10, 12);
        assert_eq!(p.desired(&view(S, &views, 4, 0)), 6);
        // Warming replicas count toward what is already decided.
        assert_eq!(p.desired(&view(2 * S, &views, 4, 2)), 6);
        // A small step bounds the move.
        let mut p = policy(0.7, 1, 30.0);
        assert_eq!(p.desired(&view(S, &views, 4, 0)), 5);
    }

    #[test]
    fn scale_down_waits_out_the_cooldown_after_a_scale_up() {
        let mut p = policy(0.7, 8, 30.0);
        let busy = fleet(4, 10, 12);
        assert_eq!(p.desired(&view(10 * S, &busy, 4, 0)), 6);
        // Load gone: 6 replicas at 0.1 want ceil(6 * 0.1 / 0.7) = 1, but not yet.
        let idle = fleet(6, 1, 10);
        assert_eq!(p.desired(&view(20 * S, &idle, 6, 0)), 6, "inside the cooldown");
        assert_eq!(p.desired(&view(41 * S, &idle, 6, 0)), 1, "after the cooldown");
    }

    #[test]
    fn the_bounds_win() {
        let mut p = policy(0.7, 100, 0.0);
        let views = fleet(4, 10, 12);
        let mut v = view(S, &views, 4, 0);
        v.max = 5;
        assert_eq!(p.desired(&v), 5);
        let idle = fleet(4, 0, 12);
        let mut v = view(2 * S, &idle, 4, 0);
        v.min = 3;
        assert_eq!(p.desired(&v), 3);
    }

    #[test]
    fn with_nothing_serving_the_fleet_is_left_where_it_is() {
        let mut p = policy(0.7, 8, 30.0);
        let views = fleet(0, 0, 8);
        assert_eq!(p.desired(&view(S, &views, 0, 2)), 2);
    }
}
