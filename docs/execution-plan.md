# Execution plan

Proposal, awaiting Issao. Nothing here is started.

Two things shape it. The fastest possible local loop matters more than anything else, because
this project is a research instrument and its value is proportional to how many experiments get
run. And the deployment target is Google Cloud, which should change almost nothing about how the
thing is built.

---

## 0. The three decisions that make or break iteration speed

**One process, threads not processes, for all development.** The Ingress-to-Leaf boundary is
real and is defined in `leaf.proto`, but in development it is a Rust trait call, not a gRPC hop.
Measured: a process boundary costs 50-100 microseconds per synchronisation barrier against a
0.2-1 millisecond lookahead, which breaks the 20x realtime target outright. So the fast path is
also the simple path. gRPC appears only where a process boundary genuinely exists, which is
Frontend to Ingress.

**Headless first, dashboard last.** A command that runs a scenario and prints a scorecard is a
one-second feedback loop. A dashboard is a thirty-second one, and it cannot be built usefully
until the engine produces data worth displaying. The first five milestones have no frontend at
all.

**Scenarios in protobuf text format, not TOML.** `Scenario` is already a proto message, and
prototxt is human-readable, comment-friendly, and needs no converter or second schema. An
earlier note proposed TOML; that would mean maintaining a mapping and two places for a default
to drift. If hand-authoring prototxt proves annoying in practice, add a TOML front end then, not
now.

---

## 1. Milestones

Each has a definition of done that is observable, not "it works". The ordering is chosen so that
something end-to-end exists as early as possible, rather than building layers that only meet at
the end.

### M0. Workspace and CI — half a day

Cargo workspace with the crates from `docs/ARCHITECTURE.md` section 10.8, `prost` and `tonic`
codegen wired up, no logic. CI runs `cargo test --workspace`, `cargo clippy -- -D warnings`,
`protoc` validation, `tools/check_diagram.py`, and `bench/validate_epochs.py`.

**Done:** a green CI run on an empty workspace, and a dependency test asserting that
`sim-ingress` cannot reach `sim-model` or `sim-physics`. That last one matters: if Ingress can
compute replica physics, eventually it will, and the layer boundary erodes.

### M1. Walking skeleton — the first real milestone

**The point is to get one dynamic end to end before building anything in depth.** Target
dynamic 1 from `docs/ARCHITECTURE.md` section 12: the round-robin rolling hotspot with
heterogeneous request sizes. It needs no LLM-specific modelling at all, since `disable_decode`
turns the workload into stateless serving, which is exactly why it is first.

Contains: the event queue, absolute epoch clock, seeded RNG streams, a replica with a queue and
a service time, round-robin and random routing, arrivals with size heterogeneity, latency
histograms, and a static HTML report.

**Done:** `cargo run -p sim-run -- run scenarios/rr_hotspot.prototxt` produces a report in which
p99 latency under round-robin is visibly worse than under random at the same offered load, with
the fleet below its rated capacity. Two runs with the same seed produce identical fingerprints.

That is a genuinely useful result on day two or three, and it exercises every layer.

### M2. Physics — the load-bearing milestone

The cost model with `max(bandwidth, compute)`, KV accounting, continuous batching with analytic
epochs, chunked prefill, preemption with swap or recompute, and the referee.

**Done:** the differential test passes. Both the naive per-step advance Issao asked for and the
closed-form advance run over randomised batches and agree on every completion time, every KV
occupancy at a boundary, and every event count. `bench/validate_epochs.py` already proves the
mathematics in Python; this is the same property in Rust, in CI, on every change to the cost
model. Plus the calibration anchors: batch-1 decode within 5% of 10.25 ms, and batch-256
throughput within 15% of the measured 8,773 tokens per second.

### M3. Three-layer split — one day

Introduce the Leaf shard boundary as a trait, with the conservative barrier from
`docs/ARCHITECTURE.md` section 10.5. Still one process.

**Done:** the determinism-across-shards test. The same scenario at 1, 4 and 16 shards produces
byte-identical fingerprints. **This is the tripwire for the entire parallel design** and it
should be written the same day the boundary is, not later.

### M4. Workload and telemetry

Arrival processes, session and prefix structure, the delayed telemetry path, and trace replay
through `WorkloadSource`. Calibrated against `docs/calibration.md`.

**Done:** replay of the Azure 2024 trace reproduces its own measured index of dispersion at both
one-second and one-minute windows, and the offered-load report matches what was requested. The
second half matters as much as the first: a generator that quietly falls behind flatters every
policy in the run.

### M5. Policies — the first real fan-out

The policy trait, the referee in strict mode, and a set: round-robin, random, least-requests,
least-KV, power-of-two-choices, prefix-affinity, and a token-bucket admission controller.

**Done:** an A/B report ranking them on goodput at fixed seed and workload, plus the per-decision
cost measurement showing each is O(1) or O(log N). A policy that only works at small fleet size
is worse than none, because its conclusion will not transfer.

This is where parallel implementers become safe: one policy per work unit, same trait, different
files.

### M6. Failures and the collapse scenarios

Failure injection including gray failure, client retry with budgets, and the two collapse modes:
preemption cascade and retry storm.

**Done:** a scenario that demonstrably fails to recover after offered load returns to normal,
with `metastable_collapse` set, and a companion scenario with a retry budget that does recover.
The contrast is the deliverable, not either run alone.

### M7. Control analysis

The perturbation input and the frequency sweep, producing gain and phase from offered load to
observed queue depth.

**Done:** an empirical Bode plot from which the oscillation onset of a proportional controller is
*predicted*, then confirmed by a separate run at that frequency. This is the most distinctive
thing in the project and it should not be deferred to the end.

### M8. Dashboard

`sim-ingress` over gRPC-web, and the React app: the interactive playground and the scenario
showcase.

**Done:** a scenario runs, streams, pauses, rewinds and resumes in a browser without
re-simulating on a scrub.

### M9. Scale validation

**Done:** 6,250 replicas at 2x realtime or better, measured, with the memory figures from section
1.5 confirmed and the event queue at or under 100 ns per event.

---

## 2. Local development loop

The loop to optimise is edit, test, see a number change.

```bash
# Inner loop, sub-second on the small scenarios used for tests
cargo test -p sim-physics

# Run a scenario and read a scorecard, a few seconds
cargo run --release -p sim-run -- run scenarios/rr_hotspot.prototxt

# A/B two policies at the same seed
cargo run --release -p sim-run -- compare scenarios/rr.prototxt scenarios/p2c.prototxt

# Sweep a parameter
cargo run --release -p sim-run -- sweep scenarios/base.prototxt \
    --set policies.routing.name=round_robin,p2c,least_kv

# Everything CI checks, before pushing
cargo test --workspace && cargo clippy -- -D warnings \
  && python3 bench/validate_epochs.py && python3 tools/check_diagram.py
```

Rules that keep it fast:

- **`--release` for anything measuring time.** A debug build makes the cost model look wrong in
  ways that waste an afternoon.
- **Test scenarios are tiny**: ten replicas, ten simulated seconds. Correctness does not need
  scale, and a test suite that takes a minute stops being run.
- **No Docker in the loop.** Containers exist for deployment only.
- **Frontend, when it exists:** `vite dev` against a locally running `sim-ingress`, with
  `tonic-web` so no Envoy proxy is needed anywhere.

---

## 3. Deployment

### 3.1 The ladder

| Stage | Shape | When |
|---|---|---|
| Local single process | Ingress and Leaf as threads, CLI | M1 onward, and forever for development |
| Local with frontend | plus `sim-ingress` serving gRPC-web, `vite dev` | M8 |
| Container, run locally | one image, same binary | before first deploy |
| **Cloud Run** | one container per concurrent run | first cloud target |
| GKE Autopilot | only if a single run must span machines | probably never |

**Cloud Run is the right first cloud target, and possibly the only one.** A simulation run is a
long-lived stateful thing holding roughly 1.3 GB, so it wants to stay on one instance for its
life. Cloud Run gives that with session affinity, CPU always allocated, and a max-instances cap
that maps directly onto Issao's note about ten backend replicas: ten instances means ten
concurrent runs. It needs no Kubernetes, no node pools, and no Envoy, because `tonic-web` speaks
gRPC-web natively.

GKE only becomes necessary if one simulation must span machines. Section 1.4's measurement says
one core reaches 20x realtime for the whole fleet, so that day should not come.

### 3.2 First deployment, concretely

```bash
PROJECT=lbsim-dev
REGION=us-central1
gcloud config set project "$PROJECT"
gcloud services enable run.googleapis.com artifactregistry.googleapis.com \
    cloudbuild.googleapis.com storage.googleapis.com

# Image registry
gcloud artifacts repositories create lbsim --repository-format=docker --location="$REGION"

# Results outlive instances, so a finished run is still readable after scale-to-zero
gsutil mb -l "$REGION" "gs://$PROJECT-runs"

# Build. A two-stage Dockerfile: cargo build --release, then a distroless runtime image.
gcloud builds submit --tag "$REGION-docker.pkg.dev/$PROJECT/lbsim/sim:$(git rev-parse --short HEAD)"

# Deploy. The flags that matter are explained below; none is incidental.
gcloud run deploy lbsim \
  --image "$REGION-docker.pkg.dev/$PROJECT/lbsim/sim:$(git rev-parse --short HEAD)" \
  --region "$REGION" \
  --cpu 4 --memory 4Gi \
  --no-cpu-throttling \
  --session-affinity \
  --min-instances 0 --max-instances 10 \
  --concurrency 4 \
  --timeout 3600 \
  --set-env-vars "RESULTS_BUCKET=gs://$PROJECT-runs" \
  --no-allow-unauthenticated

# Frontend: a static bundle, served from the bucket behind Cloud CDN
gsutil mb -l "$REGION" "gs://$PROJECT-web" && gsutil web set -m index.html "gs://$PROJECT-web"
```

Why each non-obvious flag:

- **`--no-cpu-throttling`.** Cloud Run throttles CPU between requests by default. A simulation
  advances between requests, so throttling would pause it whenever the browser is quiet.
- **`--session-affinity`.** A run lives in one instance's memory. Without affinity a subscription
  can land on an instance that has never heard of it.
- **`--concurrency 4`.** Concurrency here is simulation runs per instance, not HTTP requests per
  second. Four runs on four vCPUs, one core each, matching the measured budget.
- **`--timeout 3600`.** Streaming subscriptions are long-lived; the default hour is the ceiling
  and the client should reconnect rather than assume more.
- **`--no-allow-unauthenticated`.** Nothing here should be public. Add Identity-Aware Proxy when
  more than one person uses it.
- **`--min-instances 0`.** Scale to zero when idle. A cold start is a container pull, seconds, not
  the minutes a real inference replica takes.

### 3.3 What must be true in the code for that to work

Worth stating now, because retrofitting any of it is unpleasant:

1. **A completed run's results go to Cloud Storage, not local disk.** An instance can vanish.
2. **The Ingress must be able to say "I do not hold that run".** Session affinity is best-effort,
   so the client needs a clean error and a way to find the results in the bucket instead.
3. **No filesystem state that must survive.** Scenarios come in over the wire or from the image;
   snapshots live in memory or in the bucket.
4. **A `/healthz` that reports readiness without touching a run**, or a slow run marks the
   instance unhealthy.
5. **Structured logs to stdout**, since that is what Cloud Logging reads.

### 3.4 Deliberately not in scope

Multi-region, autoscaling on anything other than instance count, a database, and Terraform. Five
`gcloud` commands are easier to read than a state file, and if this ever needs more than that, it
will also need a different plan.

---

## 4. Testing strategy

Four layers, each catching what the others cannot:

1. **Unit tests.** Cheap and local.
2. **The differential oracle.** Naive per-step advance versus closed form, on randomised inputs,
   in CI on every cost-model change. This is the one that catches the class of bug that produces
   plausible wrong numbers, which is the class that would destroy the project's value. A
   true-division bug in the Python reference broke the exact-arithmetic proof while every
   float-based check still passed; only exact arithmetic noticed.
3. **Determinism fingerprints.** Same scenario and seed produce identical event counts and
   checksums, across shard counts, across thread counts.
4. **Golden scenarios.** A handful of scenarios with recorded scorecards. A change that moves a
   number has to say why. Not exact equality on floats; a tolerance and a stated reason.

Plus the standing rule from `docs/agent-architecture.md`: every quantitative claim in a document
must be reproducible by a script in `bench/`, or marked as an estimate. Two of my own claims this
session were wrong and both were caught by measurement rather than by reasoning.

---

## 5. Sequencing and what blocks what

```
M0 workspace ──> M1 walking skeleton ──> M2 physics ──> M3 shard split
                                            │              │
                                            └──> M4 workload ──> M5 policies ──> M6 failures
                                                                      │
                                                                      └──> M7 control analysis
M5 ──> M8 dashboard ──> M9 scale validation
```

- M1 through M4 are coupled and should be sequential, with review between. Parallel agents on
  coupled work produce plausible-looking integration failures.
- M5 is the first safe fan-out: one policy per work unit against a frozen trait.
- M8 can start once `ingress.proto` and `subscription.proto` are frozen, which they now are,
  though it should wait until M6 so there is a dynamic worth watching.

**Blocked on Issao:** the interfaces are reviewed and folded in, so M0 can start on his word. The
open questions in `TASKS.md` all have defaults and none of them blocks.

---

## 6. Honest risks in this plan

1. **M2 is where a subtle error becomes invisible.** The differential oracle is the mitigation and
   it is not optional.
2. **M4's realism is bounded by what is public.** `docs/calibration.md` measures a prefix-reuse
   ceiling of 0.37 to 0.55 on production traces against the 0.9 that benchmark numbers imply, and
   the sharing topology is unmeasured entirely. Conclusions that depend on it must be reported as
   ranges, not points.
3. **M8 can consume unbounded time.** It should be a separate workstream against a frozen
   interface, and it should not start before M6.
4. **Cloud Run's session affinity is best-effort.** Section 3.3 item 2 is the mitigation, and it
   has to be designed in rather than discovered in production.
5. **The plan front-loads a useful result at M1 by disabling decode.** That is deliberate, but it
   means the first thing that works is not yet LLM-specific. Worth saying out loud so nobody
   mistakes the skeleton for the product.
