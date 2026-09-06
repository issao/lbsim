# TASKS — things that need Issao

Last updated: 2026-09-06 14:10 by Claude.

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

## 0. Read this first: a subagent changed your billing configuration

**What happened.** `gcloud` in this sandbox is authenticated as your own Google account with full
permissions, not a scoped service account. A research pass used it to provision real resources, and
while removing a duplicate budget it had created, its filter also matched **your** `lbsim monthly cap`
budget and deleted it. It recreated one at $50 with alerts at 50, 90 and 100 percent, rebuilt from the
name and amount rather than from a copy.

**Verified state, checked directly rather than taken on report:**

| | |
|---|---|
| Budgets on the lbsim billing account | `lbsim monthly` at $100, `lbsim monthly cap` at $50, both with 3 thresholds |
| Projects linked | `lbsim-gcp` and `lbsim-prod` |
| Cloud Run services running | **none**, so nothing is currently spending |

- [x] **Confirm the $50 budget matches what you had**, since it was rebuilt from its name and amount
      and any other settings on it were lost.
- [x] **Decide whether you want two projects.** `lbsim-gcp` and `lbsim-prod` are both linked to
      billing. If one is redundant, unlinking it removes a way to be surprised.
      Issao: Lets just keep one of them, I don't care which.
- [ ] **Consider revoking this sandbox's credential when today is done.** `gcloud auth revoke
      issaofujiwara@gmail.com`. Until then, every agent here can do anything you can.
      Issao: Sounds good. Leave me a task to do that once you are done with what you need. If there is a way to downgrade your permission while still allowing claude to deploy in this project, lets do that.

**What has changed on Claude's side.** `CLAUDE.md` now forbids any mutating cloud command without your
explicit per-action approval, forbids touching billing at all, forbids delete filters that are not
exact matches, and forbids delegating cloud work to a subagent unless it is read-only. That last rule
is the one that would have prevented this: a subagent inherits these credentials and cannot be
supervised mid-action.
// Issao: I want claude to be able to interact with my gcp account, there is nothing of significance there. Just try to keep under a $50/month budget for now.

This is a larger issue than the access-token question below, and it points the same way: the safe
pattern is that you run mutating commands and Claude prepares everything else.
// Issao: For now, I would like claude to be able to make mutating commands to the deployment as we iterate.

## 1. Give Claude deploy powers without billing powers — four commands

You asked for this specifically. It is achievable as a **hard boundary** rather than a promise, and
the trick is that Claude currently has *too much* access, so the fix is to take some away.

### Verified state, checked read-only

| | |
|---|---|
| Fully provisioned project | **`lbsim-gcp`** — has the `lbsim` registry, the `lbsim-gcp-runs` bucket, Cloud Run enabled, and the deploy account |
| `lbsim-prod` | registry and Cloud Run enabled, but **no** deploy account and no results bucket. Probably redundant |
| `lbsim-deployer@lbsim-gcp` roles | `run.admin`, `artifactregistry.writer`, `cloudbuild.builds.editor`, `iam.serviceAccountUser`, `monitoring.viewer` |
| Billing roles on that account | **none**, on either project. Confirmed by filtering the IAM policy |

So the account you want already exists and already cannot touch billing. What is missing is that this
sandbox is authenticated as *you*, which overrides all of it.

### The four commands

Run each with a leading `!` so it executes here and the key is written straight to disk. **The key must
never pass through this conversation**; a transcript is stored, and a credential in one is disclosed
rather than transient.

```bash
# 1. Let the deploy account write run results. Skip if already granted.
gcloud storage buckets add-iam-policy-binding gs://lbsim-gcp-runs \
  --member=serviceAccount:lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com \
  --role=roles/storage.objectAdmin

# 2. Create the key directly into the sandbox, never via the chat.
gcloud iam service-accounts keys create /home/agents/.config/gcloud/lbsim-deployer.json \
  --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com --project=lbsim-gcp

# 3. Switch this sandbox to that identity.
gcloud auth activate-service-account --key-file=/home/agents/.config/gcloud/lbsim-deployer.json
gcloud config set project lbsim-gcp

# 4. The step that makes it a boundary rather than a convention.
gcloud auth revoke issaofujiwara@gmail.com
```

Issao: I have done all stesp except the last.

**Step 4 is the one that matters.** Without it, steps 1 to 3 are a preference Claude could undo. With
it, the sandbox holds only an identity that has no billing role, no project IAM, and no
`storage.admin`, so touching your budgets becomes impossible rather than merely forbidden.

It only removes the credential *in this sandbox*. Your other machines are unaffected. If you ever need
owner-level access here again, `gcloud auth login`.

### Blast radius, stated plainly

The key is a long-lived secret sitting on disk. What it can do: deploy Cloud Run services and jobs,
push container images, run Cloud Builds, write to one bucket, and read monitoring. What it cannot do:
change billing, change IAM, delete buckets, or touch any other project. Worst case is compute burned
inside the instance caps.

- [ ] Run the four commands
- [ ] When today is done, delete the key: `gcloud iam service-accounts keys list --iam-account=...`
      then `keys delete KEY_ID`
- [ ] Decide whether to unlink `lbsim-prod` from billing, since it is half-provisioned and duplicating
      it is a way to be surprised

## 1b. Where the TXT record goes in Porkbun

Search Console's **Domain** property needs the TXT at the apex, which is why the host is left empty.

1. Log in to Porkbun, go to **Domain Management**.
2. Find `lbsim.ai` and click **DNS** on that row. That opens *Edit DNS Records*.
3. In the **Add a DNS record** form at the top of that page:

| Field | Value |
|---|---|
| **Type** | `TXT` |
| **Host** | **leave completely empty.** Porkbun shows `.lbsim.ai` beside the box; empty means the apex. Do not type `@` |
| **Answer** | the whole `google-site-verification=…` string, pasted exactly |
| **TTL** | leave the default, 600 |

4. Click **Add**.
5. Wait a minute, then click **Verify** in Search Console. Porkbun's DNS propagates quickly.

**Do not delete existing TXT records** while doing this. Multiple TXT records at the apex are normal and
expected; anything for mail or domain policy must stay.

The A and AAAA records for the apex are a separate step and only needed once a service exists to point
at. Claude will confirm the exact addresses against what the domain mapping returns rather than from a
published list.

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
