//! Deadline-aware admission: the cheap failure instead of the expensive one.
//!
//! `common.proto` `Outcome` ranks failures by what they cost the fleet. A rejection at the router is
//! free: the request never touched a replica. A timeout while running is the expensive one: the
//! replica spent steps producing tokens the client had already stopped waiting for. Under overload
//! `accept_all` turns most excess load into the expensive failure; `deadline_aware` is meant to turn
//! it into the cheap one, and to do so *before* the request costs anything. These tests pin that
//! ordering and the two sanity conditions around it: an idle fleet sheds nothing, and more headroom
//! sheds more.

mod common;

use common::*;
use lbsim::policy;
use lbsim::scenario::Scenario;
use lbsim::sim;

/// The overload fixture: 1.4x rated capacity with a short client timeout so the fleet is offered
/// more than it can serve, and a request that waits too long is a visible failure within the run.
fn overload(admission: &str) -> Scenario {
    let mut s = at_load(1.4);
    s.client_timeout_s = 10.0;
    s.admission = admission.into();
    s
}

#[test]
fn deadline_aware_is_registered_and_labelled() {
    let mut sc = Scenario::default();
    sc.admission = "deadline_aware".into();
    sc.admission_headroom = 0.25;
    let p = policy::make_admission(&sc).expect("deadline_aware must resolve by name");
    assert_eq!(p.label(), "deadline_aware(headroom=0.25)");
}

#[test]
fn nothing_is_shed_when_the_fleet_is_idle() {
    let mut sc = at_load(0.2);
    sc.admission = "deadline_aware".into();
    let r = sim::run(&sc).unwrap();
    assert!(r.records.len() > 100, "the run produced too few records to mean anything");
    assert_eq!(r.outcome("rejected"), 0, "an idle fleet shed requests that would have made their deadline");
}

#[test]
fn overload_sheds_early_instead_of_timing_out_late() {
    let base = sim::run(&overload("accept_all")).unwrap();
    let dl = sim::run(&overload("deadline_aware")).unwrap();

    let (base_run, dl_run) = (base.outcome("timeout_running"), dl.outcome("timeout_running"));
    let (base_rej, dl_rej) = (base.outcome("rejected"), dl.outcome("rejected"));
    assert_eq!(base_rej, 0, "accept_all rejected something");
    assert!(
        base_run > 0,
        "the overload fixture produced no running timeouts under accept_all; it is not an overload"
    );
    assert!(
        dl_run < base_run,
        "deadline_aware timed out {dl_run} running requests against accept_all's {base_run}; \
         shedding early is not preventing the expensive failure"
    );
    assert!(dl_rej > base_rej, "deadline_aware shed nothing under overload");
}

#[test]
fn goodput_is_not_worse_under_overload() {
    let base = sim::run(&overload("accept_all")).unwrap().goodput_tokens_s();
    let dl = sim::run(&overload("deadline_aware")).unwrap().goodput_tokens_s();
    assert!(
        dl >= base,
        "goodput fell from {base:.0} to {dl:.0} tokens/s under deadline_aware; shedding what could \
         not finish should free capacity for what can"
    );
}

#[test]
fn headroom_zero_admits_what_headroom_half_sheds() {
    let mut zero = overload("deadline_aware");
    zero.admission_headroom = 0.0;
    let mut half = zero.clone();
    half.admission_headroom = 0.5;
    let (rz, rh) = (sim::run(&zero).unwrap(), sim::run(&half).unwrap());
    let (z, h) = (rz.outcome("rejected"), rh.outcome("rejected"));
    assert!(
        z < h,
        "headroom 0 rejected {z} and headroom 0.5 rejected {h}; more reserved headroom must shed more"
    );
}
