# Interfaces

Every interface in the system is defined here. Two categories, deliberately kept in one
place because the wire diagram references both.

## A. Simulated interfaces — the modelled system's own APIs

`request.proto`, `serving.proto`, `telemetry.proto`, `kv.proto`, `capacity.proto`

These describe the RPCs a real LLM serving fleet would expose: client to gateway, gateway
to router, router to replica, replica to telemetry, and so on. Each arrow in
`docs/diagrams/system.drawio` names one `service.Method` from these files.

**They are not served over real gRPC inside the simulator.** Doing so would cost microseconds
per call and defeat the scale target. Instead:

- The **message types** are generated and used as the simulator's internal payloads, so the
  modelled payload matches what a real system carries. Fields that would cost bytes on a
  real wire cost bytes here too, which keeps telemetry-volume questions honest.
- The **service definitions** are the contract the simulator's components obey, and the
  anchor the diagram points at. Every modelled call pays a configured latency, jitter, and
  loss probability drawn from the scenario.
- A future adapter can serve these for real against a live system, which is the
  out-of-scope-for-v1 calibration loop in VISION section 5. Defining them properly now is
  what keeps that door open.

## B. Product interfaces — the simulator's own service

`ingress.proto`, `leaf.proto`, `subscription.proto`, `metrics.proto`, `scenario.proto`

Served for real. These describe the three-layer deployment in `docs/ARCHITECTURE.md` section 10:

- `ingress.proto` — Frontend to Ingress. Every call is O(1) in payload: run lifecycle, speed,
  bounded step, rewind, and subscription management.
- `leaf.proto` — Ingress to Leaf. `Leaf.Advance` is the conservative synchronisation barrier, and
  it carries dispatched work, tier grants, control actions, completions, control telemetry,
  metrics and tier requests in a single round trip. One message per shard per window is what
  makes the barrier affordable.
- `subscription.proto` — shared by both observability hops, deliberately, so the O(1) bound is
  enforced once. A subscription declares a **wall-clock budget** rather than only a simulated
  sampling interval, because naming an interval alone lets the wire rate rise with simulation
  speed, which is the failure this interface exists to prevent.

## C. The policy boundary

`policy.proto`

Neither quite. It is the seam between engine and policy, expressed in proto so that a
policy can eventually be written in another language or another process, and so that the
staleness contract is visible in the type rather than enforced by convention.

## Conventions

- Package `lbsim.v1`. Breaking changes go to `v2`; within `v1` only additions.
- **`Truth`, in `request.proto`, must never appear in anything a policy observes.** It holds the
  real output length. It travels on the Ingress-to-Leaf hop because the Leaf owns the physics and
  needs it to advance an epoch, and nowhere else.
- **Simulated time is `uint64` nanoseconds**, never a float. Floats accumulate rounding and
  make byte-identical reproducibility fragile. Intermediate math may use `f64`; stored and
  transmitted time may not.
- Durations are `uint64` nanoseconds with a `_ns` suffix. Token counts are `uint32` with a
  `_tokens` suffix. Byte counts are `uint64` with a `_bytes` suffix. Rates carry their unit.
- Ids are `uint64` dense handles, not strings. At 112,500 requests/s a string id per request
  would dominate allocation.
- Every enum has a `_UNSPECIFIED = 0` member, so an unset field is detectable.
