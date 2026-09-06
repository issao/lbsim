# TASKS — things that need Issao

Last updated: 2026-09-06 16:00 PDT by Claude.

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

## 1. Domain: delegation fixed, verification can proceed now

The service is live and public at <https://lbsim-irpwc2yaoa-uc.a.run.app>. The domain is only a nicer
address for it, so do this when convenient.

You repointed the registrar and it has already taken: at 15:37 both Google (`8.8.8.8`) and Cloudflare
(`1.1.1.1`) resolve `lbsim.ai` through `*.ns.porkbun.com` and return your TXT record. If your own
machine still shows the Google nameservers, that is its cache; Search Console asks Google, and Google
sees it now. The history, kept because it explains the earlier failure:

```
$ dig +short NS lbsim.ai
ns-cloud-d1.googledomains.com. ns-cloud-d2.googledomains.com. ns-cloud-d3.googledomains.com. ns-cloud-d4.googledomains.com.
$ dig TXT lbsim.ai @ns-cloud-d1.googledomains.com | grep status
;; ->>HEADER<<- opcode: QUERY, status: REFUSED
```

The registrar still delegates `lbsim.ai` to Google Cloud DNS, and the zone that lived there was in
`lbsim-prod`, which you removed. Those nameservers now refuse every query, so `lbsim.ai` resolves to
nothing at all. Earlier today it pointed at Firebase Hosting for `lbsim-prod`; that is gone too. This is
also why the Porkbun TXT record did not work: Porkbun's DNS was not authoritative for the domain.

- [x] **Step 0, fix the delegation.** Done by you, and live at 15:37:
      `dig +short NS lbsim.ai @8.8.8.8` → `fortaleza.ns.porkbun.com.` and three more;
      `dig +short TXT lbsim.ai @8.8.8.8` → `"google-site-verification=8sYf…"`.
- [ ] **Step 1, verification.** Go to Search Console now and press Verify on the `lbsim.ai` Domain
      property. The TXT it wants is already served.
- [ ] **Step 2, the mapping.** Domain ownership is per account, and the deploy account is not an owner.
      Either add `lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com` as an Owner of the `lbsim.ai`
      property in Search Console, after which Claude does the rest, or run it yourself once:
      `gcloud beta run domain-mappings create --service=lbsim --domain=lbsim.ai --region=us-central1`
- [ ] **Step 3, the apex records.** The command prints four `A` and four `AAAA` records. Add them in
      Porkbun with the Host field empty, and delete Porkbun's two parking `A` records on the bare host
      first (`207.207.210.107` and `.229`, present at 15:37).
      The certificate follows on its own, in fifteen minutes to a few hours.

Why a domain mapping and not a load balancer: the mapping and its certificate are free, and a load
balancer costs about $18 a month in forwarding rules before serving a byte. Estimate, not measured.

**If you do nothing:** `lbsim.ai` shows Porkbun's parking page and the service stays on its `run.app` URL.

## 2. One arena rule change still open

`docs/arena.md` section 5b has the argument, `docs/arena-implementation.md` the numbers.

| Change | Why | Default until you say |
|---|---|---|
| Score the minimum of goodput as a **share of offered work**, not absolute goodput | the minimum is otherwise always set by the lightest load, so a load generator would win by proposing trivial loads | raw objective kept; the share is reported beside it |

The SLA cap is answered twice over, 0.95 for now with the aim of improving the SLO target later; the
design task for a better target is routed to the tech lead above.

**If you do nothing:** the default stands, and every score records the rule set it was earned under.

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
