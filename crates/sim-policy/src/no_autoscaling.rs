//! lbsim-policy: autoscaling names=none
//! A fixed fleet: exactly `replicas` up for the whole run, which is what every run did before the
//! seam existed. The engine schedules no autoscaling tick for this policy, so a scenario that names it
//! is byte-identical to one written before the key existed.

use crate::autoscaling::{AutoscalingPolicy, FleetView};
use sim_scenario::Scenario;

pub struct NoAutoscaling;

pub fn make(_sc: &Scenario) -> Box<dyn AutoscalingPolicy> {
    Box::new(NoAutoscaling)
}

impl AutoscalingPolicy for NoAutoscaling {
    fn label(&self) -> String {
        "none".into()
    }
    fn desired(&mut self, v: &FleetView) -> usize {
        v.ready + v.warming
    }
}
