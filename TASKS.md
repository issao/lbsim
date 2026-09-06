# TASKS — things that need Issao

Last updated: 2026-09-06 14:10 by Claude.

Tick a box when you have reviewed it. Anything Claude can do alone is not in this file; see
`STATUS.md` for what is done and live.

---

## 0. Done, and one consequence you should know about

**The deploy credential is in place and the boundary is real.** Verified by attempting to break it,
not by reading role names:

| Check | Result |
|---|---|
| Credentials in the sandbox | only `lbsim-deployer@lbsim-gcp`; your account is revoked from here |
| Read Cloud Run, registry, results bucket | all work |
| List billing accounts | denied |
| Read or modify project IAM | denied, cannot even read the policy it would need to change |
| **Grant itself a role** | **denied**, tested with a real attempt |

**The consequence: Claude can no longer check your budget.** That is the trade you asked for, and it is
the right one, but it means budget verification is now yours alone. Last confirmed at 14:23, while
Claude still had owner access:

| Budget | Amount | Thresholds |
|---|---|---|
| `lbsim monthly` | $100 | 3 |
| `lbsim monthly cap` | $50 | 3 |

Both on billing account `015B1A-AA7EAB-107FD2`, both project-filtered. One command to re-check, as
yourself on any machine:

```bash
gcloud billing budgets list --billing-account=015B1A-AA7EAB-107FD2 \
  --format="table(displayName, amount.specifiedAmount.units, thresholdRules.len())"
```

Worth doing once, because the `$50` one was rebuilt by a subagent from its name and amount after being
deleted, so any other settings it carried are gone.

You noted mid-way that all steps but the last were done. All four are done now, and the revoke is what
made the boundary real: verified by a real attempt to grant the deploy account a role, which was refused
because it cannot even read the policy it would need to modify.

### Checking the budget in the console, without the CLI

Since Claude can no longer see billing, this is the path you will want. It is not under the project.

1. Go to `console.cloud.google.com`.
2. Open the navigation menu, top left, and choose **Billing**. If you have several billing accounts it
   will ask which; pick **My Billing Account - lbsim** (`015B1A-AA7EAB-107FD2`).
3. In the left sidebar of the billing page, click **Budgets & alerts**.

You should see two rows:

| Name | Amount | Alerts |
|---|---|---|
| `lbsim monthly` | $100 | 3 thresholds |
| `lbsim monthly cap` | $50 | 3 thresholds |

// Issao: Confirmed.

Click either to see its thresholds, its scope, and who gets email. Two things worth confirming while you
are there: that each one is **scoped to a project** rather than the whole billing account, and that the
alert email addresses are ones you actually read. An alert nobody sees is the same as no alert.

Direct link, which skips the account picker:
`console.cloud.google.com/billing/015B1A-AA7EAB-107FD2/budgets`

**A budget alerts, it does not stop spending.** Google will email at each threshold and keep serving. If
you want a hard stop you need a Cloud Function on the Pub/Sub budget notification that disables billing,
which is drastic and can take a project down. The instance caps in the deploy plan are the practical
control; the budget is the tripwire behind them.

For watching actual usage rather than the limit, the number to look at is **instance-hours** on the Cloud
Run service page. Anything non-zero while nobody is using the dashboard is a bug in the idle shutdown,
not a pricing surprise.

- [x] Re-check the two budgets with the command above
- [ ] Delete the deploy key when this phase ends:
      `gcloud iam service-accounts keys list --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com`
      then `gcloud iam service-accounts keys delete KEY_ID --iam-account=...`. The list command shows
      the id, so it does not need recording here
- [ ] Decide whether to unlink `lbsim-prod` from billing. It has a registry and Cloud Run but no deploy
      account and no results bucket, so it is half-provisioned and duplicating it is a way to be surprised

## 0b. One grant needed to finish the deploy — one command

Everything for the deploy is written, committed and verified except this. Cloud Build refuses with:

> The user is forbidden from accessing the bucket [lbsim-gcp_cloudbuild] ... or if the user has the
> "serviceusage.services.use" permission

The message is misleading. It is not really about the bucket: I retried with a bucket the deploy account
does own and got the same error. The missing permission is `serviceusage.services.use`, which Cloud
Build needs in order to attribute API usage to the project.

**The error suggests Service Usage Admin. Do not grant that** — it can enable and disable APIs. The
minimal role is Service Usage Consumer, which only permits *using* APIs that are already enabled:

```bash
gcloud projects add-iam-policy-binding lbsim-gcp \
  --member=serviceAccount:lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com \
  --role=roles/serviceusage.serviceUsageConsumer
```

That still grants nothing over billing, nothing over IAM, and no ability to turn APIs on or off. It is
the smallest thing that unblocks a build.

- [ ] Run the command above. Claude will retry the deploy immediately and report the URL.

Everything else is ready: `Dockerfile`, `.dockerignore`, `deploy.sh`, and a static server with a health
check that touches no run state and path handling verified against both plain and percent-encoded
traversal. The image generates all six reports at build time, so it ships real results.

The service will be **private**. Viewing it is
`gcloud run services proxy lbsim --project lbsim-gcp --region us-central1 --port 8080`. Say so if you
would rather it were public and I will flip one flag; it is mock data and published findings, so the
risk is low, but that is your call rather than mine.

## 1. Domain linking — three steps, and one gotcha worth knowing before you start

Nothing here blocks a first deploy. Cloud Run hands out a `run.app` URL that needs no domain, so the
service can be live and working before any of this. Do it when convenient.

### The gotcha: domain ownership is per-account, and Claude is now a different account

Creating a Cloud Run domain mapping requires the *calling* account to be a verified owner of the
domain. You verified `lbsim.ai` as yourself. The deploy service account is a different identity and is
not an owner, so **Claude cannot create the mapping** even though it can list and manage mappings
otherwise. Confirmed: listing works, and the verified-owner check is what would refuse.

Two ways round it, and the second is simpler:

- **Add the service account as a domain owner.** In Search Console, open the `lbsim.ai` property →
  Settings → Users and permissions → Add user, and enter
  `lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com` with the **Owner** role. Search Console accepts
  service account addresses. After that Claude can create and manage the mapping unattended.
- **Create the mapping yourself**, one command, once. Claude does everything else.

### Step 1 — verify the domain, if not already done

`search.google.com/search-console`, add a **Domain** property for `lbsim.ai`, and put the TXT record it
gives you in Porkbun. Domain Management → the **DNS** button on the `lbsim.ai` row → the *Add a DNS
record* form:

| Field | Value |
|---|---|
| Type | `TXT` |
| Host | **leave completely empty**, not `@`. Porkbun appends the domain itself |
| Answer | the whole `google-site-verification=…` string |
| TTL | leave the default |

Do not remove existing TXT records; several at the apex is normal and anything for mail must stay.

Issao: I tried this but it didn't work, maybe I need to wait to propagate and then continue.

### Step 2 — create the mapping, after a service exists

Either grant the service account ownership above and Claude runs this, or run it yourself:

```bash
gcloud beta run domain-mappings create --service=lbsim --domain=lbsim.ai --region=us-central1
```

It prints the DNS records to add. **Use what it prints rather than any published list**, since the
addresses depend on the region and the mapping.

### Step 3 — the apex records in Porkbun

Same *Add a DNS record* form, and again with **Host left empty**. Expect four `A` records and four
`AAAA` records.

Before adding them: **delete Porkbun's default parking record on the bare host**, and make sure no URL
forwarding or redirect is enabled on the apex. Either will fight the mapping. Then Google issues the
certificate automatically, which takes fifteen minutes to a few hours with nothing to do but wait.

### Why a domain mapping rather than a load balancer

A domain mapping is free and the managed certificate is free. A global load balancer costs roughly $18
a month in forwarding rules before serving any traffic, which is a fifth of your budget for nothing,
and a hosting rewrite in front of Cloud Run risks buffering server-streamed responses, which would
break the live charts in a way that looks like the simulation hanging.

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
