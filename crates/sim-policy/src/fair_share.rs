//! lbsim-policy: admission names=fair_share
//! Weighted fair share over tenants, in tokens.
//!
//! `scenario.proto` `AdmissionPolicy.FairShare`. What is being shared is the fleet's *serviceable*
//! token throughput, not the queue: once the fleet is contended, every tenant is entitled to its
//! weight's share of the rated capacity, and may burst to `burst_multiplier` times that before being
//! shed. Below contention nobody is being denied anything, so there is nothing to be fair about and
//! the policy admits everything. That gate is what lets an unused share be picked up by whoever wants
//! it: a lone tenant on an otherwise idle fleet is never shed, whatever its weight.
//!
//! Accounting is a sliding window of tokens per tenant. A request is charged at admission, when its
//! output length is unknown, as `prompt_tokens` plus the scenario's mean output; the completion
//! callback corrects the charge to the real output. Rejected requests are charged nothing, because
//! they consumed nothing.

use crate::{Admission, AdmissionContext, AdmissionPolicy};
use sim_core::{Nanos, SECOND};
use sim_scenario::Scenario;
use std::collections::VecDeque;

/// The accounting window. Long enough that one long-context prompt (24k tokens in the baseline
/// mixture) is a small fraction of any tenant's budget, so a single request cannot flip a tenant into
/// shedding; short enough that a burst is forgiven well inside a client timeout, so a tenant is not
/// still paying for it after its users have given up.
const WINDOW: Nanos = 10 * SECOND;

/// One signed token movement. Admission charges are positive; a completion correction has the sign
/// of the estimate's error.
struct Charge {
    at: Nanos,
    tenant: usize,
    tokens: i64,
}

pub struct FairShare {
    burst: f64,
    /// The scenario's mean output over the mixture: the unbiased prior on a length nobody can observe
    /// at admission. Corrections therefore sum to about zero over many requests, which is why a
    /// correction landing after its charge has aged out of the window is harmless rather than a leak.
    output_estimate: i64,
    /// Rated tokens per window, prompt and output together, from the scenario's cost model. This is
    /// the pie that is shared; admitted tokens beyond it are queueing, not being served.
    window_capacity: f64,
    charges: VecDeque<Charge>,
    per_tenant: Vec<i64>,
    total: i64,
}

pub fn make(sc: &Scenario) -> Box<dyn AdmissionPolicy> {
    let (p_mean, o_mean) = sc.mixture_means();
    let window_s = WINDOW as f64 / SECOND as f64;
    Box::new(FairShare {
        burst: sc.fair_share_burst,
        output_estimate: o_mean.round() as i64,
        window_capacity: sc.rated_rps() * (p_mean + o_mean) * window_s,
        charges: VecDeque::new(),
        per_tenant: Vec::new(),
        total: 0,
    })
}

impl FairShare {
    fn expire(&mut self, now: Nanos) {
        while let Some(c) = self.charges.front() {
            if c.at + WINDOW > now {
                break;
            }
            self.per_tenant[c.tenant] -= c.tokens;
            self.total -= c.tokens;
            self.charges.pop_front();
        }
    }

    fn charge(&mut self, at: Nanos, tenant: usize, tokens: i64) {
        if tokens == 0 {
            return;
        }
        if self.per_tenant.len() <= tenant {
            self.per_tenant.resize(tenant + 1, 0);
        }
        self.per_tenant[tenant] += tokens;
        self.total += tokens;
        self.charges.push_back(Charge { at, tenant, tokens });
    }

    /// Clamped at zero: a downward correction can outlive the charge it corrects.
    fn held(&self, tenant: usize) -> f64 {
        self.per_tenant.get(tenant).copied().unwrap_or(0).max(0) as f64
    }
}

impl AdmissionPolicy for FairShare {
    fn label(&self) -> String {
        format!("fair_share(burst={})", self.burst)
    }

    fn admit(&mut self, ctx: &AdmissionContext<'_>) -> Admission {
        self.expire(ctx.now);
        let tenant = ctx.request.tenant as usize;
        // An empty weight vector is the scenario saying there is no tenancy; a tenant the scenario
        // did not weight has nothing to be measured against. Both are the no-op case.
        let share = match ctx.tenant_weights.get(tenant) {
            Some(&s) => s,
            None => {
                self.charge(ctx.now, tenant, ctx.request.prompt_tokens as i64 + self.output_estimate);
                return Admission::Admit;
            }
        };
        let contended = self.total as f64 > self.window_capacity;
        let entitlement = self.burst * share * self.window_capacity;
        if contended && self.held(tenant) > entitlement {
            return Admission::Reject;
        }
        self.charge(ctx.now, tenant, ctx.request.prompt_tokens as i64 + self.output_estimate);
        Admission::Admit
    }

    fn on_complete(&mut self, tenant: u32, output_tokens: u32, now: Nanos) {
        self.expire(now);
        self.charge(now, tenant as usize, output_tokens as i64 - self.output_estimate);
    }
}
