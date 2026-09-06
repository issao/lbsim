# Execution plan

**Scope for today is `docs/scope-today.md`.** Issao asked for a plan finishable by 16:30 against an
eight-hour total budget, of which about three hours went on design, so roughly seven eighths of what
follows is deferred. That document ranks every dynamic by cost, proposes three packages that fit, and
recommends one. This file remains the plan for the whole project.

**Where this stands at 15:40.** What was built today diverges from the milestones as written, and
the divergences are recorded here rather than rewritten out of the plan:

- One crate, `lbsim`, with no dependencies, rather than the workspace of `docs/ARCHITECTURE.md`
  section 10.8. `prost` and `tonic` are not wired; nothing speaks the protos over a wire yet.
- Scenarios are `key = value` text files in `scenarios/`, not prototxt. Section 0's argument for
  prototxt still holds once the proto codegen exists; until then the text format costs nothing.
- M1's target was reached and passed: six dynamics reproduce, in `docs/findings.md`. M0.5 is done
  under `web/`. M6's retry contrast is done. The arena's mechanical half is done ahead of order,
  because Issao asked for it.
- The deploy is `deploy.sh` and `docs/deploy.md` when it lands, not section 3.4: project `lbsim-gcp`,
  a static server for the reports, public at Issao's decision, `--max-instances 10`. The budgets set
  are $100 and $50, not the single $50 of section 3.6. The reasoning in 3.1 to 3.7 stands.

Two things shape it. The fastest possible local loop matters more than anything else, because
this project is a research instrument and its value is proportional to how many experiments get
run. And the deployment target is Google Cloud, which should change almost nothing about how the
thing is built.

---

## 0. The three decisions that make or break iteration speed

**Overridden by Issao, 16:05:** *"Leaf shards should become separate processes in a sharded
server."* The paragraph below is kept as the measured cost of that decision; the realtime target
must be re-measured against process barriers rather than assumed.

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

Issao asked for a stand-in dashboard early, so he can observe progress: *"STart building a stand in
dashboard for now just so that I can observe progress."* His full dashboard specification is now
`docs/ui-spec.md`.

That reprioritisation is folded into the milestones below as **M0.5**, and it is a better idea than
the original ordering. The stand-in runs against mock data generated in the browser, so it is
buildable before any engine exists, and it front-loads exactly the mistakes that are expensive to
find late: a wrong tab structure, a layout that does not fit the panels, a pagination story that does
not survive contact with the wire protocol. Every panel carries a visible "mock data" marker until it
is wired to a real run, because a dashboard that looks real while showing invented numbers is how
someone ends up trusting a chart that was never connected.

The original argument for putting the dashboard last still holds for the *real* dashboard, which
still lands at M8. What moves early is the shell, not the wiring.

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

### M0.5. Stand-in dashboard — one day, in parallel

The shell of the load-test dashboard from `docs/ui-spec.md` section 5, against mock data generated
in the browser. Homepage, both panels with all tabs, the A/B view, and the showcase with one real
walkthrough script so the format gets exercised.

Runs in parallel with M0 and M1 because it shares no files with them, which makes it the one piece
of genuinely safe early fan-out.

**Done:** Issao can open it, click through every tab, and say what is wrong with the layout before
any of it is wired to a server. Every panel visibly marked as mock.

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

### M8. Dashboard, and the first deploy

`sim-ingress` over gRPC-web, and the React app: the interactive playground and the scenario
showcase. This is also the milestone that first deploys, so it carries the seven code requirements
in section 3.5, of which idle self-shutdown and resumable checkpoints are the two that cost money
if skipped.

**Done:** a scenario runs, streams, pauses, rewinds and resumes in a browser without re-simulating
on a scrub. And, on Cloud Run: closing the browser tab drives instance count to zero within the
idle window, verified on the metrics page rather than assumed.

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

Constraint from Issao: **no always-on.** Scale to zero when idle, scale up on demand, within a
replica budget starting at ten. That constraint drives the design rather than being a flag on the
end of it.

### 3.1 Why scale-to-zero is the whole cost story

Approximate Cloud Run pricing in `us-central1`, with CPU always allocated. **Verify against
current rates before relying on these**; the ratio is the point, not the absolute.

| Shape | Cost |
|---|---|
| 4 vCPU, 4 GiB, one hour of active simulation | **~$0.38** |
| One warm instance held always, per month | **~$274** |
| Ten-minute interactive session | ~$0.06 |
| Sweep of 20 variants, 5 minutes each, 2 vCPU, as a Job | ~$0.32 total |
| Moderate use: 20 interactive hours plus 30 sweeps per month | **~$18** |

A single always-warm instance costs more than a month of real use. So the only cost decision that
matters is whether anything is running when nobody is looking, and everything below serves that.

### 3.2 Two shapes, because interactive and batch have opposite cost profiles

**Interactive: a Cloud Run service, min-instances 0.**

CPU must be *always allocated* while an instance exists, not request-scoped, because a simulation
advances between requests and request-scoped CPU would freeze it mid-run. That is not the same as
always-on: with `--min-instances 0` there is no instance at all when nothing is running, and
nothing is billed.

The catch, and it is the important one: **Cloud Run keeps an instance alive while a request is in
flight, and a streaming subscription is a request in flight.** A forgotten browser tab would
therefore pin an instance indefinitely and quietly cost $274 a month. The mechanism that prevents
it already exists in the interfaces: subscriptions are time-leased. Combined with bounded stream
lifetimes and client reconnect, an abandoned tab stops renewing, the last stream ends, the run
checkpoints to Cloud Storage, and the instance is reaped. Section 3.5 item 2 makes that a
requirement rather than a hope.

**Batch: Cloud Run Jobs, which cannot idle by construction.**

A parameter sweep is embarrassingly parallel and finite. A Job runs tasks to completion and
exits, billing only for execution, with no idle window to pay for and no scale-to-zero behaviour
to get wrong. `--parallelism` is the replica budget, directly:

```bash
gcloud run jobs execute lbsim-sweep --tasks 20 --parallelism 10
```

That is strictly cheaper than a service for the same work, and it is where most compute will
actually go, because comparing policies is the primary use.

### 3.3 What "autoscale on the scale of the simulation" should mean

Worth separating two axes, because only one of them is worth autoscaling.

**More concurrent work: yes, autoscale, and it is nearly free.** Many runs, many users, a sweep of
twenty variants. Cloud Run scales instances on demand and Jobs scale on `--parallelism`. The
replica budget is a hard cap in one flag: `--max-instances 10` on the service,
`--parallelism 10` on Jobs.

**One larger simulation: no, and this is worth saying plainly.** The measurement in
`docs/ARCHITECTURE.md` section 1.4 is that a single core carries the whole 6,250-replica fleet at
20x realtime, and one core reaches 2x with roughly fiftyfold headroom. So a bigger simulation does
not need more instances; it needs a bigger instance, and even then rarely. Spreading one run
across processes would cost 50-100 microseconds per synchronisation barrier against a lookahead of
a fraction of a millisecond, which loses more than the parallelism gains.

So the rule is: **scale on queued runs, never on the size of a run.** If a single scenario ever
does exceed one instance, the first move is `--cpu 8` rather than a second process, and the second
move is to revisit `docs/ARCHITECTURE.md` section 10.5 with a measurement in hand.

A sizing ladder, so a large run gets a large instance without holding one when idle:

| Fleet in the scenario | CPU / memory | Notes |
|---|---|---|
| under 500 replicas | 1 vCPU, 1 GiB | tests and small scenarios |
| 500 to 10,000 | 2 vCPU, 2 GiB | one cluster |
| 10,000 to 50,000 | 4 vCPU, 4 GiB | the target fleet |
| stretch, 500,000 | 8 vCPU, 16 GiB | verify against section 1.5 first |

Ingress chooses from the scenario at submission time and picks the matching Job or revision. All of
them still scale to zero.

### 3.4 The commands

```bash
PROJECT=lbsim-dev
REGION=us-central1
gcloud config set project "$PROJECT"
gcloud services enable run.googleapis.com artifactregistry.googleapis.com \
    cloudbuild.googleapis.com storage.googleapis.com

gcloud artifacts repositories create lbsim --repository-format=docker --location="$REGION"

# Results outlive instances, so a finished run is readable after scale-to-zero.
gsutil mb -l "$REGION" "gs://$PROJECT-runs"
# Delete run artefacts after 90 days so storage does not creep.
printf '{"rule":[{"action":{"type":"Delete"},"condition":{"age":90}}]}' > /tmp/lc.json
gsutil lifecycle set /tmp/lc.json "gs://$PROJECT-runs"

TAG="$REGION-docker.pkg.dev/$PROJECT/lbsim/sim:$(git rev-parse --short HEAD)"
gcloud builds submit --tag "$TAG"

# --- interactive service: scales to zero, capped at 3 instances -------------
gcloud run deploy lbsim \
  --image "$TAG" --region "$REGION" \
  --cpu 4 --memory 4Gi \
  --no-cpu-throttling \
  --cpu-boost \
  --session-affinity \
  --min-instances 0 \
  --max-instances 3 \
  --concurrency 4 \
  --timeout 900 \
  --set-env-vars "RESULTS_BUCKET=gs://$PROJECT-runs,IDLE_SHUTDOWN_SECONDS=300" \
  --no-allow-unauthenticated

# --- batch sweeps: no idle cost at all, replica budget is --parallelism -----
gcloud run jobs create lbsim-sweep \
  --image "$TAG" --region "$REGION" \
  --cpu 2 --memory 2Gi \
  --task-timeout 3600 \
  --max-retries 1 \
  --parallelism 10 \
  --set-env-vars "RESULTS_BUCKET=gs://$PROJECT-runs" \
  --command /usr/local/bin/sim-run --args sweep

# --- frontend: a static bundle, no compute at all --------------------------
gsutil mb -l "$REGION" "gs://$PROJECT-web" && gsutil web set -m index.html "gs://$PROJECT-web"
```

The flags that are load-bearing rather than incidental:

| Flag | Why |
|---|---|
| `--min-instances 0` | the entire cost story; nothing runs and nothing bills when idle |
| `--max-instances 3` | the replica budget for interactive use, deliberately below ten so sweeps have room |
| `--no-cpu-throttling` | a simulation advances between requests; request-scoped CPU would freeze it. Not always-on |
| `--cpu-boost` | faster cold start, which matters because every session now starts cold |
| `--session-affinity` | a run lives in one instance's memory |
| `--concurrency 4` | runs per instance, not requests per second: four runs on four vCPUs, one core each |
| `--timeout 900` | **fifteen minutes, not an hour.** A bounded stream forces reconnect, so an abandoned tab cannot pin an instance |
| `--parallelism 10` | the replica budget for batch, where the compute actually goes |

### 3.5 What must be true in the code

Retrofitting any of these is unpleasant, so they belong in the milestone that first deploys.

1. **Results go to Cloud Storage, not local disk.** An instance can vanish at any time.
2. **Idle self-shutdown.** With no live subscription lease and no queued work for
   `IDLE_SHUTDOWN_SECONDS`, checkpoint the run to the bucket and stop advancing so the instance can
   be reaped. **This is the single most important cost control**, because without it a forgotten
   browser tab costs more per month than all intentional use.
3. **A resumable checkpoint.** Reopening a run after scale-to-zero restores from the bucket. The
   snapshot machinery in `docs/ARCHITECTURE.md` section 8.2 already exists for rewind; this is the
   same mechanism with a different destination.
4. **A clean "I do not hold that run".** Session affinity is best-effort, so a client must be told
   to reconnect or to read finished results from the bucket instead of receiving a confusing error.
5. **Bounded streams with client reconnect**, matching `--timeout 900`. The lease renewal in
   `subscription.proto` is the natural place.
6. **Health check that does not touch a run**, or a busy instance is marked unhealthy and killed
   mid-simulation.
7. **Structured logs to stdout**, which is what Cloud Logging reads.

### 3.6 Budget guardrails, belt and braces

```bash
# Hard ceiling on spend visibility: alert at 50%, 90% and 100% of a monthly budget.
gcloud billing budgets create \
  --billing-account "$BILLING_ACCOUNT" \
  --display-name "lbsim monthly" \
  --budget-amount 50USD \
  --threshold-rule percent=0.5 --threshold-rule percent=0.9 --threshold-rule percent=1.0
```

Four layers, none of which relies on remembering to turn something off:

1. `--min-instances 0` on every service. Nothing idles.
2. `--max-instances` and `--parallelism` cap concurrent compute at the replica budget.
3. Application-level idle shutdown, section 3.5 item 2, so a live stream cannot pin an instance.
4. A billing budget alert, because the first three are code and code has bugs.

A useful habit alongside them: `gcloud run services describe lbsim --format='value(status.traffic)'`
and the Cloud Run metrics page show instance-hours, which is the number to watch. Anything non-zero
while nobody is using it is a bug in item 3.

### 3.7 Deliberately not in scope

Multi-region, a database, Terraform, and GKE. Committed spend and reservations are actively wrong
here, since they are the opposite of scale-to-zero. If this ever needs more than the commands above
it will also need a different plan.

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

Nothing is blocked on Issao. The open questions in `TASKS.md` all have defaults.

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
4. **A live subscription keeps a Cloud Run instance alive.** Left unhandled, one forgotten browser
   tab costs more per month than all deliberate use. Section 3.5 item 2, application-level idle
   shutdown, is the mitigation and it is the single most important cost control in the plan.
   Session affinity being best-effort is the secondary version of the same problem, handled by
   item 4.
5. **The plan front-loads a useful result at M1 by disabling decode.** That is deliberate, but it
   means the first thing that works is not yet LLM-specific. Worth saying out loud so nobody
   mistakes the skeleton for the product.
