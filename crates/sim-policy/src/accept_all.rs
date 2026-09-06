//! lbsim-policy: admission names=accept_all
//! No admission control. The baseline every other admission policy is compared against.

use crate::{Admission, AdmissionContext, AdmissionPolicy};
use sim_scenario::Scenario;

pub struct AcceptAll;

pub fn make(_sc: &Scenario) -> Box<dyn AdmissionPolicy> {
    Box::new(AcceptAll)
}

impl AdmissionPolicy for AcceptAll {
    fn label(&self) -> String {
        "accept_all".into()
    }
    fn admit(&mut self, _ctx: &AdmissionContext<'_>) -> Admission {
        Admission::Admit
    }
}
