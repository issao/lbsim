# TASKS — things that need Issao

Last updated: 2026-09-06 16:05 PDT by Claude.

Stack ranked, most blocking first. Every item says what Claude does if you say nothing, so nothing
here stalls the work. What is finished and live is in `STATUS.md`.

## Routed to the tech lead

Five design decisions from Issao at 15:26, recorded in the design of record (`docs/ARCHITECTURE.md`
section 14, `docs/arena.md`, `docs/execution-plan.md`) by the housekeeping agent. Each has code
consequences that belong to the tech lead; verbatim, from `TASKS.md` before the markers were removed:

1. *"SLA 0.95 ok for now, but we need to figure out how to do better."* Open design task: per-class
   or length-scaled first-token targets so a 0.99 cap becomes reachable; `docs/arena.md` §5b.
2. *"Arena policy generator should actually have full power to write code to write new policies, as
   well as tuning parameters on existing policies."* A policy candidate is code against the policy
   trait, not only a `PolicySpec`; `docs/arena.md` §6. The proto side is done: `GeneratedPolicy`
   variant in every policy slot, 947649b. Left for the tech lead: the policy registry that resolves it
   by name from one file per policy.
3. *"Re prefix sharing topology. I don't, assume some reasonable distributions of lengths of session
   and how often they fork off and merge back new agents and create a distribution based on that."*
   The workload model derives the prefix tree from a session process: session length, fork-off rate,
   merge-back rate, each swept; `docs/calibration.md` §9 row and `docs/ARCHITECTURE.md` §14.
4. *"Leaf shards should become separate processes in a sharded server."* Overrides the threads
   recommendation in `docs/ARCHITECTURE.md` §10.5 and `docs/execution-plan.md` §0. The barrier cost
   measurement stands; the realtime target is to be re-measured against processes.
5. *"If cluster memory tiers are owned by ingress, it can just tell the leaf if it is in DRAM/SSD or
   not. We can save a lot of resources if we need to by turning this into a bloom filter tuned
   appropriately using the global seed, preserving an acceptible false negative cache miss rate that
   is deterministic and also realistic from a machine loss perspective."* Ingress-owned tiers
   confirmed, with the residency hint as a seeded bloom filter; `docs/ARCHITECTURE.md` §10.6.

---

## 1. Domain: all three steps done by you; the certificate is Google's to issue now

Checked at 16:04: the mapping for `lbsim.ai` exists in `lbsim-gcp` (created 15:58), Google's resolvers
return the four Cloud Run `A` records (`216.239.32.21` and three more), `http://lbsim.ai` already
redirects to `https://`, and the mapping reports `CertificatePending`: *"Certificate issuance
pending."* Nothing to do but wait; Google retries every five minutes, and it usually takes fifteen
minutes to a few hours. Claude will re-check and move this to the answered log when `https://lbsim.ai`
serves.

One optional gap: no `AAAA` records are served. The mapping printed four; without them IPv6-only
clients cannot reach the domain, and everyone else can. Add them in Porkbun when convenient, Host
empty, one record per address.

- [x] Add the four `AAAA` records, optional.

**If you do nothing:** `lbsim.ai` serves over IPv4 once the certificate lands; IPv6 clients get the
`run.app` URL only.

## 2. One arena rule change still open

`docs/arena.md` section 5b has the argument, `docs/arena-implementation.md` the numbers.

| Change | Why | Default until you say |
|---|---|---|
| Score the minimum of goodput as a **share of offered work**, not absolute goodput | the minimum is otherwise always set by the lightest load, so a load generator would win by proposing trivial loads | raw objective kept; the share is reported beside it |

The SLA cap is answered twice over, 0.95 for now with the aim of improving the SLO target later; the
design task for a better target is routed to the tech lead above.

**If you do nothing:** the default stands, and every score records the rule set it was earned under.

Issao: You can remove this, I agreed with this.

## 3. Decisions with a default

| Decision | Default | Rework if changed later |
|---|---|---|
| A routing policy that scans the fleet fails the run rather than warning (`docs/ARCHITECTURE.md` §10.4) | fail | small |
| Prefix-affinity index at Ingress is a bounded top-K, not exact (§10.4) | bounded | small |

## 4. Later, when this phase ends

- [ ] Delete the deploy key. `gcloud iam service-accounts keys list --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com`
      shows the id, which starts `94afd556`; then `keys delete KEY_ID --iam-account=...`.
- [ ] Grant `roles/logging.viewer` to the deploy account **only when a container fails on startup**.
      Today it cannot read container or request logs; nothing has needed them yet, and `docs/deploy.md`
      records the gap. Not worth granting pre-emptively.

- [ ] Delete the leftover test image, which the deploy account cannot delete itself:
      `gcloud artifacts docker images delete us-central1-docker.pkg.dev/lbsim-gcp/lbsim/lbsim:wstest --project lbsim-gcp --quiet`.
      If you do nothing it sits in the registry costing cents.
- [ ] Two more grants to hold back until needed, both from `docs/deploy.md`: `storage.buckets.update`
      for the 90-day delete rule on `gs://lbsim-gcp-runs`, which matters once runs write results there;
      and `artifactregistry.repoAdmin`, or running the tear-down as yourself, when this phase ends.

**If you do nothing:** the key stays until you delete it, the first startup crash is undiagnosable
until you grant the role, and the results bucket keeps everything it is ever given.

---

## Answered, kept for the record

- 16:03, domain: registrar repointed to Porkbun, Search Console verified, mapping created, apex `A`
  records added, all by you. Certificate pending at 16:04, item 1.

- 15:56, *"for the dashboard, add alink to the reports from the homepage."* Done by the tech lead,
  cf12e33, on `master` at 15:59; live after the next `./deploy.sh`.

- 15:26, five design decisions: SLA cap 0.95 for now; the arena policy generator writes code for new
  policies as well as tuning parameters; prefix-sharing topology is derived from a session model with
  fork-off and merge-back rates; Leaf shards are separate processes in a sharded server; memory tiers
  are Ingress-owned with a seeded bloom-filter residency hint. Recorded in `docs/ARCHITECTURE.md` §14.
- `lbsim-prod` removed by you at 15:26. It hosted the `lbsim.ai` DNS zone, see item 1.

- Budgets: `lbsim monthly` $100 and `lbsim monthly cap` $50 on billing account `015B1A-AA7EAB-107FD2`,
  confirmed by you in the console at 15:15. Claude can no longer see billing; re-check as yourself with
  `gcloud billing budgets list --billing-account=015B1A-AA7EAB-107FD2`.
- Deploy credential: `lbsim-deployer@lbsim-gcp` only, no billing, cannot read or grant IAM, verified by
  a real attempt. Service Usage Consumer granted by you at 15:15. The deploy went through at 15:22 and
  needed no other grant: the real blocker was two bucket permissions, worked around in `cloudbuild.yaml`.
- The service is **public**: mock data and published findings, nothing sensitive.
- Scope today: package B of `docs/scope-today.md` plus item 7, retry storm last, side missions in
  parallel. *"Don't cut the react dashboard"* and *"Start executing on the Arena story"*: both built.
- Staffing: three long-lived agents with disjoint file ownership, recorded in `CLAUDE.md`.
- Reviews ticked: execution plan, arena, ui-spec, agent architecture, ARCHITECTURE, calibration.
- Design: analytic epoch advancement, request cohorts parked, prefix caching in phase 2, tiers pooled
  per cluster, three-layer deployment, absolute epoch time in `uint64`, a 70-billion-parameter model
  on eight H100s as reference hardware, the arena objective as specified, scale to zero within ten
  replicas, no always-on. All in `docs/ARCHITECTURE.md` section 14.
