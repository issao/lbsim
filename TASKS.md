# TASKS — things that need Issao

Last updated: 2026-09-06 14:05 by Claude.

Tick a box when you have reviewed it. Anything Claude can do alone is not in this file; see
`STATUS.md` for what is done and live.

---

## Before you leave: read one file

**`docs/scope-today.md`, about ten minutes.** It ranks all seventeen dynamics from your vision by what
each needs and costs, shows the total is about 22 hours against three remaining, and recommends
package B. If you disagree with that package, it is the only decision that changes what happens while
you are out.

`STATUS.md` has what runs now, the first result, and the queue of work Claude will do next.

## Short answer: nothing blocks starting

You asked what is blocked on you to finish the execution plan and start building. **Nothing.**

All twenty-five of your instructions are folded in, the interfaces are reviewed and simplified, and
the four remaining decisions all have defaults recorded. Claude can start now with:

- **M0**, the cargo workspace and CI. Half a day, and it commits to nothing later milestones cannot
  change.
- **M0.5**, the stand-in dashboard you asked for, against mock data. One day, shares no files with
  M0, and it exists precisely so you can see progress and criticise the layout before anything is
  wired up.

Everything in the review list below can happen in parallel with those two. Only section 3 items
would cause rework if they arrive late, and each says how much.

**Say "go" and Claude starts on M0 and M0.5.**

---

## 1. HARD BLOCKER — four things only you can do, for the cloud deployment

Everything else is unblocked. These four are not, and one of them cannot be delegated at all.

`gcloud` and `gsutil` are already installed here and can reach Google APIs, so the tooling side is
ready. Docker is absent and does not matter, because Cloud Build builds remotely.

**Simpler architecture than the plan sketched.** One Cloud Run service serving both the static
frontend and the API, mapped straight to your domain. No Firebase, no load balancer, no second
component. Two reasons at this budget: a global load balancer costs about $18 a month in forwarding
rules before any traffic, a fifth of the budget for nothing; and a hosting rewrite in front of Cloud
Run risks buffering server-streamed responses, which would break the live charts in a way that looks
like the simulation hanging. A direct domain mapping avoids both and the certificate is free.

- [ ] **Create the project and link billing.** Then tell Claude the project id.
- [ ] **Verify domain ownership.** Google requires a signed-in human for this; it cannot be
      delegated. About five minutes in Search Console.
- [ ] **Add the DNS records** the domain mapping returns, at your registrar. Four A and four AAAA for
      the apex.
- [ ] **Create the budget.** Budgets live on the billing account, and granting billing access to save
      two minutes is a bad trade.

```bash
gcloud projects create lbsim-prod --name=lbsim
gcloud billing projects link lbsim-prod --billing-account=YOUR_BILLING_ID
gcloud services enable run.googleapis.com artifactregistry.googleapis.com \
    cloudbuild.googleapis.com storage.googleapis.com --project=lbsim-prod
gcloud domains verify YOUR_DOMAIN
gcloud billing budgets create --billing-account=YOUR_BILLING_ID \
    --display-name="lbsim" --budget-amount=100USD \
    --threshold-rule=percent=0.5 --threshold-rule=percent=0.9
```

Enabling the APIs yourself means Claude never needs the service-usage role, which is one fewer broad
permission handed over.

### The credential, and its blast radius

**A choice, and the second option is genuinely fine.** Either a service account key, or you run one
command per deploy yourself with the `!` prefix while Claude prepares everything else. A long-lived
key in a sandbox is a real secret; if you would rather not, the one-line-per-deploy path costs almost
nothing.

If you do issue a key, these roles and no others:

| Role | Why |
|---|---|
| `roles/run.admin` | deploy services and jobs, set caps, create the domain mapping |
| `roles/iam.serviceAccountUser` | act as the runtime service account |
| `roles/artifactregistry.writer` | push images |
| `roles/cloudbuild.builds.editor` | build them |
| `roles/storage.objectAdmin` | **on the results bucket only**, which you pre-create |
| `roles/monitoring.viewer` | read instance-hours, the number that catches a cost bug |

Deliberately absent: every billing role, project IAM, and `storage.admin`. Worst case if the key
leaks is compute burned inside the instance caps, not an account restructured. Rotate it when this
phase ends.

### What $100 a month buys

About **250 hours of active simulation**, at roughly $0.38 an hour for 4 vCPU and 4 GiB while a run
is actually running. Domain mapping and certificate are free; the registry is about $0.10 per GB per
month; Cloud Build has 120 free minutes a day.

The one thing that could blow it is already known and is in the plan: **a live subscription keeps an
instance alive**, so one forgotten browser tab holds an instance at about $274 a month, threefold over
budget by itself. Three defences, and Claude will not deploy without all three: zero minimum
instances, a hard instance cap, and application-level idle shutdown that checkpoints a run so the
instance can be reaped. Set the first alert at 50%, because at these rates $50 of spend means
something is wrong rather than busy.

### What Claude will write without waiting

A two-stage Dockerfile producing a distroless image, `cloudbuild.yaml`, a `deploy.sh` carrying only
the flags that are load-bearing, a health check that does not touch a running simulation, and the
idle-shutdown logic. None of that needs credentials, so it can exist before any of the above.

One note outside the cloud budget entirely: a `.ai` registration and renewal is not cheap. Worth
checking what the registrar committed you to.

## 2. Reviews — read and tick, no reply needed unless you disagree

| Done | File | Lines | Time | Why it matters |
|---|---|---|---|---|
| [x] | `docs/execution-plan.md` | 440 | reviewed | Ten milestones, the local loop, and the Cloud Run deployment. |
| [x] | `docs/arena.md` | 196 | 10 min | Your arena specification designed out. Two additions of mine are argued in sections 2.3 and 5; disagree with those if you do. |
| [x] | `docs/ui-spec.md` | 146 | 8 min | Your dashboard specification, moved out of the execution plan. Section 5 is the stand-in scope. |
| [x] | `docs/agent-architecture.md` | 268 | 12 min | How to staff this with agents. Recommends two now, not seven. |
| [x] | `docs/ARCHITECTURE.md` | 1,370 | already reviewed | Your thirteen instructions are folded in; section 14 lists every decision. Only re-read if you want to check an answer. |
| [x] | `docs/calibration.md` | 2,024 | skim section 0 | Which traces are usable, and seven corrections to the primer. The correction table at the top is the part worth reading. |

**The three things most worth disagreeing with, if you are going to disagree:**

1. **Execution plan, M1 is a walking skeleton.** It builds the round-robin rolling hotspot end to
   end with decode disabled, so the first working thing is not yet specific to language-model
   serving. Deliberate, and a real tradeoff.
2. **Arena, section 5.** I added a fixed held-out benchmark suite that no generator may touch,
   because co-evolution otherwise cannot distinguish progress from its own drift. That is an
   addition to your design, not an implementation of it.
3. **Agent architecture, section 3.** The comment watcher stays a script rather than an agent.
   Four of your seven proto instructions were invisible to it until a mechanical fix, which is an
   argument for tests rather than for judgement.

---

## 3. Decisions that would cause rework if they arrive late

Each has a default being taken, so none of them blocks. Ordered by how much rework.

### 3.1 Agent staffing — affects how work is farmed out, not what is built

`docs/agent-architecture.md` recommends two agents now, Architect and Verifier, growing to seven.
You asked for a TL and an Architect; the TL has nothing to lead until M1 exists.

**Default:** Claude works as Architect with a Verifier pass on each unit of work, and does not fan
out until M5, which is the first genuinely safe fan-out point.

**Rework if changed later:** none. Staffing can change at any milestone boundary.

### 3.2 Does the dashboard become a separate workstream? — affects scheduling

`docs/ARCHITECTURE.md` risk 7 warns it will expand without limit if it shares a backlog with the
engine. M0.5 is a one-day stand-in; M8 is the real thing.

**Default:** the stand-in now, then nothing until M6, so there is a dynamic worth watching.

**Rework if changed later:** small. Mostly a question of what Claude works on next.

### 3.3 Arena scope: tune parameters, or generate policy structures? — affects the interfaces

`docs/arena.md` section 6 item 1. A first arena that tunes parameters of hand-written policies works
with the interfaces exactly as they stand. Generating new policy *structures* needs a small
interpreted decision language, which is a real piece of work.

**Default:** parameter tuning, with the limitation stated rather than hidden.

**Rework if changed later:** moderate. The policy trait would need a second implementation, though
no proto change.

### 3.4 Prefix-sharing topology — affects what conclusions are trustworthy, not the build

`docs/calibration.md` section 5 measures a prefix-reuse ceiling of 0.37 to 0.55 on production
traces, against the 0.9 that benchmark numbers imply. What nobody publishes is the *topology*: how
many distinct system-prompt roots exist and how skewed their popularity is. It moves achievable
cache hit rate a long way.

**Do you have anything internal, even aggregated?** A histogram of requests per distinct system
prompt would be enough.

**Default:** Claude sweeps the parameter rather than fixing it, and reports affected conclusions as
ranges rather than points.

**Rework if changed later:** none to the code. It changes which conclusions can be stated as facts.

---

## 4. Answered, kept for the record

- **Analytic epoch advancement** is plan of record, and since verified numerically.
- **Request cohorts** parked with a trigger and a knob design, not rejected.
- **Prefix caching** in scope for phase 2.
- **DRAM and NVMe** fully disaggregated at cluster level; HBM modelled at three distances.
- **Three-layer deployment**, Frontend, Ingress and Leaf, is the architecture.
- **Absolute Unix epoch time**, which turned out to make `uint64` mandatory rather than preferred.
- **The five smaller decisions**: *"all of these sound good to me."*
- **Reference hardware**: a 70-billion-parameter model on eight H100s. *"We can tune later."*
- **The arena objective**: maximum goodput under an SLA cap, with policy-declared rated capacity.
- **Deployment**: scale to zero, replica budget ten, no always-on.
- **Interfaces**: reviewed across two rounds and simplified.

---

## 5. Heads up: something outside this session moves the working tree

A VS Code Git extension appears to be attached to `/home/agents/repo/lbsim`. Once it stashed
uncommitted work and switched branches, silently reverting a large document. It was recovered,
nothing was lost, and `tools/sync.sh` now warns on an unexpected stash and on the branch moving
between runs.

Nothing needed from you unless you are pointing an editor at this sandbox, in which case that is the
cause and it will recur.
