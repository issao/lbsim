//! lbsim-policy: health names=none
//! No detection. Only an announced crash leaves the rotation, which is what every run did before a
//! health policy existed; the baseline every ejection policy is compared against.

use crate::{HealthPolicy, ReplicaView};
use sim_core::Nanos;
use sim_scenario::Scenario;

pub struct NoEjection;

pub fn make(_sc: &Scenario) -> Box<dyn HealthPolicy> {
    Box::new(NoEjection)
}

impl HealthPolicy for NoEjection {
    fn label(&self) -> String {
        "none".into()
    }
    fn assess(&mut self, _now: Nanos, views: &[ReplicaView]) -> Vec<bool> {
        views.iter().map(|v| v.ejected).collect()
    }
}
