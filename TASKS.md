# TASKS — things that need Issao

Last updated: 2026-09-06 15:50 by Claude.

Stack ranked, most blocking first. Every item says what Claude does if you say nothing, so nothing
here stalls the work. What is finished and live is in `STATUS.md`.

---

## 1. Domain: the TXT record went to a DNS that does not serve `lbsim.ai`

The service is live and public at <https://lbsim-irpwc2yaoa-uc.a.run.app>. The domain is only a nicer
address for it, so do this when convenient.

You tried the Porkbun step and it did not work. It is not propagation. Checked at 15:20:

```
$ dig +short NS lbsim.ai
ns-cloud-d1.googledomains.com. ns-cloud-d2.googledomains.com. ns-cloud-d3.googledomains.com. ns-cloud-d4.googledomains.com.
$ dig +short TXT lbsim.ai @ns-cloud-d1.googledomains.com
"hosting-site=lbsim-prod"
$ dig +short A lbsim.ai @ns-cloud-d1.googledomains.com
199.36.158.100
```

The authoritative nameservers for `lbsim.ai` are **Google Cloud DNS**, not Porkbun. A record added in
Porkbun's DNS panel is never served, however long you wait. The served zone holds a Firebase Hosting
verification for `lbsim-prod` and an A record to Firebase, so `lbsim.ai` resolves to Firebase today.

Claude cannot look at that zone. The deploy account has no access to `lbsim-prod`, and the DNS API is
off in `lbsim-gcp`.

- [ ] **Step 1, verification.** Console: Network services → Cloud DNS, most likely under `lbsim-prod`,
      open the `lbsim.ai` zone, Add record set: type `TXT`, name empty, data the whole
      `google-site-verification=…` string from Search Console. Check with
      `dig +short TXT lbsim.ai @ns-cloud-d1.googledomains.com`; Cloud DNS serves it within a minute.
- [ ] **Step 2, the mapping.** Domain ownership is per account, and the deploy account is not an owner.
      Either add `lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com` as an Owner of the `lbsim.ai`
      property in Search Console, after which Claude does the rest, or run it yourself once:
      `gcloud beta run domain-mappings create --service=lbsim --domain=lbsim.ai --region=us-central1`
- [ ] **Step 3, the apex records.** The command prints four `A` and four `AAAA` records. They go in
      Cloud DNS too, and they **replace the Firebase A record**, which takes `lbsim.ai` away from
      Firebase Hosting. Say so if that is not what you want. The certificate follows on its own, in
      fifteen minutes to a few hours.

Why a domain mapping and not a load balancer: the mapping and its certificate are free, and a load
balancer costs about $18 a month in forwarding rules before serving a byte. Estimate, not measured.

**If you do nothing:** the service stays on its `run.app` URL. Everything else proceeds.

## 2. Two arena rule changes, found by running it

`docs/arena.md` section 5b has the argument, `docs/arena-implementation.md` the numbers. Both change
the objective you specified, so both are yours.

| Change | Why | Default until you say |
|---|---|---|
| Score the minimum of goodput as a **share of offered work**, not absolute goodput | the minimum is otherwise always set by the lightest load, so a load generator would win by proposing trivial loads | raw objective kept; the share is reported beside it |
| SLA cap **0.95**, or per-class first-token targets | at 0.999 every policy scores zero on every chat mixture, from prompt-length arithmetic alone | 0.95 |

**If you do nothing:** the defaults stand, and every score records the rule set it was earned under.

## 3. Decisions with a default, ordered by rework if they arrive late

| Decision | Default | Rework if changed later |
|---|---|---|
| Arena generates policy *structures*, or only tunes parameters of shipped policies (`docs/arena.md` §6) | parameter tuning | moderate: a second policy implementation, no proto change |
| Prefix-sharing topology. **Do you have anything internal, even a histogram of requests per distinct system prompt?** `docs/calibration.md` §5 measures a reuse ceiling of 0.37 to 0.55, and nothing public gives the topology | swept as a parameter; conclusions reported as ranges | none to code; changes what can be stated as fact |
| Leaf shards as threads in one process, not processes (`docs/ARCHITECTURE.md` §10.5) | threads | large, if cross-process is a hard requirement |
| A routing policy that scans the fleet fails the run rather than warning (§10.4) | fail | small |
| Prefix-affinity index at Ingress is a bounded top-K, not exact (§10.4) | bounded | small |
| Cluster memory tiers owned by Ingress; a tier operation is a modelled round trip (§10.6) | Ingress-owned | moderate |

## 4. Later, when this phase ends

- [ ] Delete the deploy key. `gcloud iam service-accounts keys list --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com`
      shows the id, which starts `94afd556`; then `keys delete KEY_ID --iam-account=...`.
- [ ] Decide what `lbsim-prod` is for. It has a registry and Cloud Run but no deploy account and no
      results bucket, and it is the project serving `lbsim.ai` through Firebase Hosting today. Unlink
      it from billing only if nothing there is wanted.

**If you do nothing:** the key stays until you delete it, and `lbsim-prod` keeps costing what it costs.

## Heads up

Something outside these sessions moves the working tree. A VS Code Git extension attached to
`/home/agents/repo/lbsim` once stashed uncommitted work and switched branches. Nothing was lost, and
`tools/sync.sh` now warns on an unexpected stash. If you are pointing an editor at this sandbox, that
is the cause and it will recur.

---

## Answered, kept for the record

- Budgets: `lbsim monthly` $100 and `lbsim monthly cap` $50 on billing account `015B1A-AA7EAB-107FD2`,
  confirmed by you in the console at 15:15. Claude can no longer see billing; re-check as yourself with
  `gcloud billing budgets list --billing-account=015B1A-AA7EAB-107FD2`.
- Deploy credential: `lbsim-deployer@lbsim-gcp` only, no billing, cannot read or grant IAM, verified by
  a real attempt. Service Usage Consumer granted by you at 15:15. The deploy went through at 15:45 and
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
