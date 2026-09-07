# Deploying lbsim

Last updated: 2026-09-07 by Claude. Owner of this file and of `deploy.sh`, `cloudbuild.yaml`,
`Dockerfile`, `.dockerignore`, `.gcloudignore`: the cloud agent. Nothing else in the repo is.

Written for you six weeks from now, having forgotten all of it. It says what is live, the one command
that redeploys it, why each flag is there, what is deliberately missing, and how to delete the lot.

---

## 1. What is live

| | |
|---|---|
| **URL** | <https://lbsim-irpwc2yaoa-uc.a.run.app> |
| Second URL, same service | <https://lbsim-1027087334969.us-central1.run.app> |
| Project | `lbsim-gcp` (number `1027087334969`) |
| Region | `us-central1` |
| Service | Cloud Run service `lbsim`, public |
| Image | `us-central1-docker.pkg.dev/lbsim-gcp/lbsim/lbsim:<git short sha>` |
| Results bucket | `gs://lbsim-gcp-runs` — also holds build source tarballs under `cloudbuild-source/` |
| Deploy identity | `lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com` |
| Runtime identity | `1027087334969-compute@developer.gserviceaccount.com` (the project default) |

It serves the dashboard at `/`, the ten generated reports at `/reports/1-routing.html` through
`/reports/10-probes.html` (the same commands as `run-demos.sh`, run at image build time), the
recorded demo runs the dashboard replays at `/runs/index.json` and `/runs/<group>/<run>/…` (see
web/README.md, "Replay mode"; 30 runs, about 15 MB, also generated at build time by
`sim-run export --demos`), and `docs/findings.md` at `/docs/findings.md`. Public deliberately: mock
data, simulated runs and published findings, nothing sensitive. Cloud Run hands out two hostnames for the same
service and both work; the second is the newer deterministic form.

## 2. The one command

```bash
./deploy.sh
```

Idempotent. It builds from your working tree, tags the image with `git rev-parse --short HEAD`,
pushes it, and deploys a revision. Running it twice on an unchanged tree rebuilds the same tag and
creates a second identical revision, which is harmless. It warns if the tree is dirty, because then
the tag names a commit that is not what is inside the image.

Override with environment variables if you ever need to: `PROJECT`, `REGION`, `SERVICE`, `REPO`,
`BUCKET`.

To check the cost control:

```bash
./deploy.sh --check-idle
```

## 3. Why the build is shaped so oddly

`deploy.sh` does not run `gcloud builds submit --tag`, which is the obvious thing, and the reason is
worth keeping because it will look like pointless complexity later.

The deploy account holds `roles/storage.objectAdmin` **on `gs://lbsim-gcp-runs` only**. It can read
and write objects there. It cannot read bucket metadata (`storage.buckets.get`) and cannot create a
bucket anywhere. `gcloud builds submit` touches Cloud Storage twice before it runs a single step:

1. **Source staging.** gcloud calls `buckets.get` on the staging bucket before uploading the context,
   and creates the bucket if absent. Both denied.
2. **Build logs.** Cloud Build defaults its log sink to `gs://lbsim-gcp_cloudbuild/logs`. That bucket
   does not exist and cannot be created by this account.

Both failures surface as the *same* error, and it names the wrong permission:

> The user is forbidden from accessing the bucket [lbsim-gcp_cloudbuild]. Please check your
> organization's policy or if the user has the "serviceusage.services.use" permission. Giving the
> user a role with this permission such as Service Usage Admin may fix this issue.

It is not about Service Usage. `roles/serviceusage.serviceUsageConsumer` is already held and made no
difference. The two causes were separated by elimination: adding `logging: CLOUD_LOGGING_ONLY` makes
the error stop naming `lbsim-gcp_cloudbuild` and start naming `lbsim-gcp-runs`, which isolates cause
1 on its own.

So instead of widening the role, both needs were removed:

* `cloudbuild.yaml` sets `options.logging: CLOUD_LOGGING_ONLY`. Logs go to Cloud Logging. No bucket.
* `deploy.sh` tars the build context, uploads it with `gcloud storage cp` — a plain object write,
  which `objectAdmin` permits — and submits with `--no-source`. Nothing is staged. The build's first
  step fetches and unpacks the tarball. Reading it needs only `objects.get`, which the Cloud Build
  service account already has through `roles/cloudbuild.builds.builder`.

**Net result: the deploy needs no permission the account did not already have.** A request for
`roles/storage.legacyBucketReader` was pending when this was written; it is no longer needed and
should not be granted. Fewer permissions is the point.

The cost of the workaround is one thing: `gcloud builds submit` cannot stream logs from Cloud
Logging, so a failed build prints nothing useful. `deploy.sh` prints the two commands to get the log
when that happens:

```bash
gcloud builds list --project lbsim-gcp --limit 1 --format='value(id)' --quiet
gcloud builds log <ID> --project lbsim-gcp --quiet
```

Do not "simplify" this back to `builds submit --tag` without re-checking those two permissions first.

### The gap this leaves: no logs

`gcloud builds log <ID>` works, but **`gcloud run services logs read` does not**:

```
ERROR: PERMISSION_DENIED: Permission denied for all log views.
```

The deploy account has no logging role, so container stdout and the request log are invisible to it.
Nothing needed them yet, because a build failure reports through Cloud Build and a serving failure
shows up as a status code. The first container that crashes on startup will need them, and the fix is
`roles/logging.viewer` on the project — worth granting *then*, with a reason, rather than now.

### One consequence: three copies of the exclude list

`.dockerignore`, `.gcloudignore` and the `tar --exclude` flags in `deploy.sh` all list the same
things (`.cargo`, `.git`, `target`, `out`, `bench`, `tests`, `web/node_modules`, `web/dist`). They
cannot be one file: `tar` does not understand gitignore syntax, and `.dockerignore` needs a negation
(`!docs/findings.md`) that the others do not. Change one, change all three.

**`.cargo` is the entry that is not about upload size, and it is the one to be careful with.**
`.cargo/config.toml` is checked in and pins an *absolute* `target-dir` of
`/home/agents/repo/lbsim/target`, so every agent's checkout and worktree shares one build directory
and cargo's own file lock serialises them. Inside a container that path is meaningless: cargo writes
the binary there instead of `./target`, and the next Dockerfile line dies with
`./target/release/sim-run: not found` — **exit 127, caused by a Cargo setting, with nothing in the
error naming Cargo.** The Dockerfile now copies the whole context (`COPY . .`) rather than an
enumerated list of source paths, because the crate layout is not stable — it became a workspace under
`crates/` — and an enumerated `COPY` breaks the day a layout change lands. That robustness is exactly
what made the `.cargo` exclusion necessary: the old enumerated `COPY` was avoiding it by accident.

Verified rather than assumed: building the workspace tree through Cloud Build failed at step 1 with
exit 127 before the exclusion and succeeded in 1m22s after it. The context is about 350 KB, so a
missed exclude also shows up as a suddenly slow upload.

Build times since, for the trend rather than as a target: 1m22s (`lbsim-00003-zc5`, single crate);
2m9s (`lbsim-00005-dzk`, 2026-09-07, workspace of five crates plus the replay dashboard, context
560 KB). The reports and the demo export are not where that went: measured against the same binary
the export takes 1.3 s and reports 7-10 take 0.4 s together. The rest is Rust and npm compile time
growing with the code, and the machine type is still the free default.

## 4. Every flag that is load-bearing

```bash
gcloud run deploy lbsim \
  --image "$TAG" --project lbsim-gcp --region us-central1 \
  --cpu 1 --memory 512Mi \
  --cpu-throttling --no-cpu-boost \
  --concurrency 80 \
  --min-instances 0 --max-instances 10 \
  --min 0 --max 10 \
  --timeout 300 \
  --allow-unauthenticated --quiet
```

| Flag | What it does, and why this value |
|---|---|
| `--min-instances 0` | The entire cost story. No instance exists when nobody is looking, and nothing is billed. |
| `--max-instances 10` | Replica budget, **on the revision**. |
| `--min 0` / `--max 10` | The same two caps **on the service**. See the trap below; these are not duplicates. |
| `--cpu 1 --memory 512Mi` | A static file server. The ten reports are 2–7 MB of HTML each and the recorded runs 15 MB of JSON; all read off disk per request, not held in memory. |
| `--cpu-throttling` | CPU only while a request is in flight. Correct and cheaper for a file server. |
| `--no-cpu-boost` | Extra startup CPU, **billed**, and on by default on new services. The container is a static binary that binds its port in milliseconds; there is nothing to accelerate. |
| `--concurrency 80` | Requests per instance. High, because serving a file is cheap, and it keeps the instance count at one under any load this will see. |
| `--timeout 300` | Request timeout. Nothing here streams, so five minutes is already generous. |
| `--allow-unauthenticated` | Public, per your instruction: mock data and published findings. |
| `--quiet` | Non-negotiable when unattended. Without it gcloud will offer to enable an API interactively and hang forever rather than failing. |

### The two-cap trap

Cloud Run has a revision-level instance cap and a service-level one, and gcloud spells them almost
identically:

* `--max-instances 10` → revision annotation `autoscaling.knative.dev/maxScale`
* `--max 10` → service annotation `run.googleapis.com/maxScale`

**The service-level cap defaults to 20 whether you ask for it or not.** The first deploy set only
`--max-instances` and the result was a revision capped at 10 inside a service capped at 20. Today's
revision is the binding constraint, so nothing was actually uncapped — but the moment anyone deploys
a revision without the flag, the service will happily scale to 20. Both are set now so the cost
control does not depend on remembering which flag is which. Verify with:

```bash
gcloud run services describe lbsim --project lbsim-gcp --region us-central1 --format=yaml --quiet \
  | grep -E 'maxScale|minScale|cpu-throttling|startup-cpu-boost'
```

Expect `run.googleapis.com/maxScale: '10'`, `autoscaling.knative.dev/maxScale: '10'`,
`cpu-throttling: 'true'`, `startup-cpu-boost: 'false'`. There is no `minScale` annotation, and its
absence *is* zero — Cloud Run omits it rather than writing `'0'`.

## 5. What is deliberately absent

| Not set | Why not, and when it will be needed |
|---|---|
| `--no-cpu-throttling` | A simulation that advances between requests needs CPU between requests. This service does not; it serves files. This becomes **required** for the real Ingress service, and `docs/execution-plan.md` 3.4 already specifies it there. |
| `--session-affinity` | A run lives in one instance's memory — again, true of Ingress, not of a file server. Nothing here holds per-user state. |
| `--execution-environment gen2` | Left at gen1, which cold-starts faster. gen2 buys a full Linux syscall surface a file server does not use. |
| Cloud Run **Jobs** | `docs/execution-plan.md` 3.2 specifies a `lbsim-sweep` Job for parameter sweeps. None exists yet. The reports are currently generated **at container build time** by the `Dockerfile`, which is why the image ships real numbers, and is enough while a sweep takes under a second. |
| A results-bucket lifecycle rule | `docs/execution-plan.md` 3.4 specifies a 90-day delete rule on `gs://lbsim-gcp-runs`. Setting it needs `storage.buckets.update`, which this account does not have, and it is not worth a grant while the bucket holds a handful of 350 KB tarballs. **It will matter** once runs write results there. |
| A minimum-instances warm pool | It is the one thing that would turn this from pennies into ~$274/month. |
| Uptime checks / alerting policies | Would need a health path that is actually reachable. See below. |
| A load balancer in front | About $18/month in forwarding rules before serving a byte, and it risks buffering server-streamed responses, which would later break live charts in a way that looks like the simulation hanging. Estimate, not measured. |

## 6. The `/healthz` trap

**`https://lbsim-irpwc2yaoa-uc.a.run.app/healthz` returns 404. The container is not at fault and the
service is healthy.** Google Front End intercepts `/healthz` on `*.run.app` hostnames and answers its
own branded error page. The request never reaches the container.

The evidence, because this looks exactly like a broken build:

| Probe | Result |
|---|---|
| `GET /healthz` on the live URL | `404`, Google's HTML error page, **no** `x-cloud-trace-context`, **no** `x-content-type-options: nosniff` |
| `GET /nope` on the live URL | `404`, body `not found`, both container headers present |
| `GET /health` and `/_ah/health` on the live URL | `404 not found` — *from the container* |
| `GET /healthz` against the same binary run locally | **`200 ok`** |
| `GET /` and `/reports/*.html` on the live URL | `200`, from the container |

So `src/serve.rs` is correct, the image is current, and every path except `/healthz` reaches the
container. Both `run.app` hostnames behave the same way.

**Why it still needs fixing.** `docs/execution-plan.md` 3.5 makes a health endpoint a requirement,
and the endpoint that exists is unreachable from outside under this hostname. Nothing is broken today
because Cloud Run's default startup probe talks to the container port directly and bypasses the
frontend. It will bite the first time an uptime check or an explicit
`--startup-probe`/`--liveness-probe` is pointed at the public URL.

**The fix is one line in `src/serve.rs`, which the cloud agent does not own:** have the health branch
match an unshadowed path as well, for instance `/health` — already verified to reach the container —
and keep `/healthz` for probes that hit the container directly. Until then, use `/` as the liveness
signal, which is what `deploy.sh` does after every deploy.

Untested, and do not assume it: whether a custom domain mapping is also subject to the shadowing.
Check it with `curl` the moment `lbsim.ai` is mapped, before relying on `/healthz` there.

## 7. Verifying the cost control, which is the only number that matters

One warm instance costs roughly **$274/month**. Everything else about this deployment costs pennies:
Artifact Registry storage, a few hundred kilobytes of tarballs, and Cloud Build minutes that fall
inside Cloud Build's free allowance on the default machine type — a build measures about 1m25s, and
the allowance is thousands of minutes a month. Check the current figure rather than trusting this one.
So there is exactly one thing to check, and checking it by reading
the flags is not checking it.

```bash
./deploy.sh --check-idle
```

It reads `run.googleapis.com/container/instance_count` for the service out of Cloud Monitoring,
aligned to 60 seconds, split by the metric's `state` label (`active` / `idle`), and prints the tail
of the series. `roles/monitoring.viewer` is enough. It uses `curl` against the Monitoring REST API
because **gcloud has no `monitoring time-series` command** — `gcloud monitoring` only has
`dashboards`, `policies`, `snoozes` and `uptime`.

### How to read the output, including the confusing case

**"no instance_count samples in the window" is normally the good answer.** Cloud Run stops emitting
the metric entirely while a service has no instances, so scale-to-zero looks like an absent series
rather than a series of zeroes. That is genuinely indistinguishable from a broken query unless you
check, so it was checked:

| Query | Result | What it proves |
|---|---|---|
| `run.googleapis.com/request_count`, same filter shape, same credential | returns real points for revision `lbsim-00001-zjv` | the credential and the filter work |
| `run.googleapis.com/nope` | HTTP 404, `Cannot find metric(s) that match type` | the API errors loudly on a bad metric rather than returning empty |
| `run.googleapis.com/container/instance_count` | `{"unit": "1"}`, no `timeSeries` key | the metric exists and has **no data**, which is the scale-to-zero signature |

There is also an ingestion lag of a couple of minutes. A check run immediately after a request shows
nothing yet; that is the pipeline, not the service. Wait two minutes and re-run.

### Doing it properly, if you want to see it with your own eyes

Send traffic, stop, and watch the series appear and then stop:

```bash
URL=https://lbsim-irpwc2yaoa-uc.a.run.app
for i in $(seq 30); do curl -sS -o /dev/null "$URL/"; done   # create an instance
# then, with no further traffic:
./deploy.sh --check-idle    # repeat every few minutes
```

### What was actually measured, 2026-09-06

Scale-to-zero was confirmed **twice, on two different revisions**, rather than inferred from the
flags. Instance count per revision, `ALIGN_MAX` over 60s, `active` and `idle` summed:

| Revision | While serving | After it stopped receiving traffic |
|---|---|---|
| `lbsim-00001-zjv` | 2 instances, 22:23–22:28 | **0 from 22:29**, one minute after the next revision took over. Series stops emitting entirely at 22:35. |
| `lbsim-00002-v7v` | 2 instances, 22:30–22:44 | 1 at 22:45, **0 from 22:47**, about four minutes after losing traffic. |
| `lbsim-00003-zc5` | 2 instances from 22:46 | still warm at the time of writing, because the deploy was being verified. |

### 2026-09-07: revision `lbsim-00005-dzk`, image `:6897543` — replay runs and reports 7-10

Build `7030c463`, 2m9s, SUCCESS. Deployed at 00:14 UTC; `GET /` answered 200 in 0.12 s. Checked from
outside, all on the container's own headers:

| Path | Result |
|---|---|
| `/runs/index.json` | 200, `application/json`, 7.9 KB, a JSON array of 30 runs in six groups (`1-routing` … `6-retry`) |
| `/runs/1-routing/least-requests/status.json`, `/runs/2-staleness/telemetry_interval_ms=100/result.json` | 200, `application/json` — nested run paths, including `=` in a segment, resolve |
| `/runs/6-retry/retry-storm-no-budget/fleet.jsonl` | 200, 986 KB, served as `application/octet-stream`: the server's type table has no `jsonl` entry. The dashboard reads it as text, so it works; a `text/plain` mapping in `crates/sim-ingress/src/lib.rs` would be tidier and is not the cloud agent's file |
| `/reports/7-no-decode.html` … `/reports/10-probes.html` | 200, `text/html`, 2.3 MB each |
| `/reports/1-routing.html`, `/docs/findings.md` | 200, unchanged |
| `/assets/index-CwoZ1XKD.js` (the dashboard bundle) | contains `replay of a recorded run` and the `runs/index.json` fetch, so `/#/dashboard` is in replay mode against the served runs |

Idle check, `./deploy.sh --check-idle` at 00:21 and again at 00:28 UTC, last self-sent request at
00:15: **1 instance throughout, never more**, still warm at 00:28. Not a scale-to-zero reading, and
not evidence against one either: the series shows `active` samples at 00:21–00:22 and 00:27 that this
session did not send (the check itself only calls the Monitoring API), so the idle clock kept
restarting — exactly the "traffic you did not send" case above. The same series also shows the
*previous* revision holding one instance from 00:03 to 00:13, before this deploy touched anything,
for the same reason. What the reading does establish: the new revision's caps are in force (see the
flag check below) and the count sat at the floor of one, so the exposure while it is being looked at
is one instance, about a cent an hour. Re-run the check on a quiet hour to see the series stop.

Flag check on the service after the deploy: `autoscaling.knative.dev/maxScale: '10'`,
`run.googleapis.com/maxScale: '10'`, no `minScale` annotation (which is zero),
`cpu-throttling: 'true'`, `startup-cpu-boost: 'false'`. Both caps held across the redeploy.

So a genuine zero, reached in one to four minutes, twice. The service total was non-zero at the end
only because of the revision that had just been deployed and probed — which is the same thing you
will see if you check immediately after running `./deploy.sh`.

**Two things that look wrong and are not.** Cloud Run started **two** instances for thirty sequential
requests at `--concurrency 80`; it is eager, it stays inside `--max-instances`, and both were reaped.
And a revision that loses its traffic to a newer revision is reaped in minutes, whereas a revision
that is simply left alone is held longer — on the order of ten to fifteen minutes — so do not read the
one-to-four-minute figure above as the idle timeout for a quiet service. What would be wrong is a
series that never ends, or any sample at all on a day nobody touched it. Either way the billing
consequence of one idle window is on the order of a cent.

**Traffic arrives that you did not send.** During this measurement requests landed at 22:31, 22:33 and
22:37 from outside the test — another agent verifying the URL, and a public `run.app` address will also
attract scanners. Each such request restarts the idle clock. It does not defeat the cost control,
because the instance is still reaped afterwards, but it does mean the instance count is not entirely
under your control, and a check run right after someone else has opened the page will show an
instance.

There is no alert on this. Adding one needs `roles/monitoring.editor`, and the tripwire behind it is
already the budget on the billing account, which this credential deliberately cannot see or change.

## 8. `lbsim.ai`: not mapped, and the DNS is currently broken

**Do not create the domain mapping yet.** Two things block it, and the second is worse than it looks.

### Blocker A: the calling account is not a verified domain owner

A Cloud Run domain mapping requires the *calling* identity to be a verified owner of the domain. You
verified `lbsim.ai` as yourself; the deploy service account is a different identity. Confirmed by
attempting it — the attempt changed nothing:

```
$ gcloud beta run domain-mappings create --service=lbsim --domain=lbsim.ai --region=us-central1
ERROR: The provided domain does not appear to be verified for the current account.
You currently have no verified domains.
```

Two ways round it. Either add `lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com` as an **Owner** of
the `lbsim.ai` property in Search Console (Settings → Users and permissions → Add user; Search
Console accepts service account addresses), after which this is unattended; or run the one command
yourself.

### Blocker B: `lbsim.ai` has no working DNS at all, as of 22:30 UTC 2026-09-06

This is the part to read before touching Search Console, because verification cannot succeed while it
is true.

`lbsim.ai`'s authoritative nameservers are Google Cloud DNS, not Porkbun. Porkbun is only the
registrar. **Any record added in Porkbun's DNS panel is never served**, which is why the TXT
verification step appeared to fail and why waiting for propagation would never have helped.

Worse, the zone those nameservers point at has stopped answering. Measured at 22:30 UTC, all four:

```
$ dig +short NS lbsim.ai
ns-cloud-d1.googledomains.com. ns-cloud-d2.googledomains.com.
ns-cloud-d3.googledomains.com. ns-cloud-d4.googledomains.com.

$ dig TXT lbsim.ai @ns-cloud-d1.googledomains.com     # and d2, d3, d4
;; ->>HEADER<<- opcode: QUERY, status: REFUSED
$ dig +short A lbsim.ai @8.8.8.8
                                                       # nothing; SERVFAIL through resolvers
```

`REFUSED` from an authoritative server means *there is no zone here for this name*. Ten minutes
earlier the same query returned `"hosting-site=lbsim-prod"` and an A record to Firebase Hosting
(`199.36.158.100`), so the zone went away during that window. The likely cause is that the zone lived
in project `lbsim-prod` and that project's billing has been detached — a read-only probe of
`lbsim-prod` returns `This API method requires billing to be enabled`.

`lbsim-prod` is the user's to decide about and is deliberately untouched here. The Cloud DNS API is
also off in `lbsim-gcp`, and enabling an API is a mutation this account will not perform.

**So the registrar's nameserver delegation has to change, whichever route you take.** It currently
points at a zone that no longer exists. You have been asked to take the first route, and it is the
simpler one:

* **Route 1, Porkbun DNS — the chosen route.** In Porkbun, Domain Management → `lbsim.ai` → the nameserver setting, and
  switch it back to Porkbun's own nameservers (use Porkbun's "use our nameservers" option rather than
  typing hostnames from memory). Porkbun's DNS panel then becomes authoritative and the original
  instructions — TXT at an empty host, then the A/AAAA records — work as written. Also delete
  Porkbun's default parking record on the bare host and make sure no URL forwarding is enabled on the
  apex; either will fight the mapping.
* **Route 2, Cloud DNS in `lbsim-gcp` — the alternative, not being taken.** Create a managed zone for `lbsim.ai` in `lbsim-gcp`, then
  set the registrar's nameservers to **the four the new zone is assigned**, which may or may not be
  the same `ns-cloud-d*` set. Do not assume they are.

Then, in order:

1. Wait for the delegation change to propagate, then confirm it before touching Search Console.
   Registrar nameserver changes take minutes to a couple of days. The check is on the NS record, not
   the TXT record, and the answer must no longer be `ns-cloud-d*`:
   ```bash
   dig +short NS lbsim.ai        # must show the new nameservers
   dig SOA lbsim.ai | grep status # must be NOERROR, not REFUSED or SERVFAIL
   ```
2. Add the Search Console `google-site-verification=…` TXT record at the apex, in whichever DNS now
   answers, and confirm it is actually served before going to Search Console:
   `dig +short TXT lbsim.ai`
3. Create the mapping — either yourself, or by Claude once the service account is an Owner:
   ```bash
   gcloud beta run domain-mappings create --service=lbsim --domain=lbsim.ai \
     --region=us-central1 --project=lbsim-gcp --quiet
   ```
4. Add **exactly the `A` and `AAAA` records that command prints**, four of each. Do not use a
   published list; the addresses depend on the region and the mapping. These replace whatever the
   apex points at, which takes `lbsim.ai` away from Firebase Hosting if it ever comes back.
5. Wait. Google issues the managed certificate automatically, fifteen minutes to a few hours, with
   nothing to do.

**If you do nothing, everything keeps working** on the `run.app` URL. The domain is only a nicer
address.

`TASKS.md` item 1 carries the same story in checklist form; this section and that item should agree,
and that item is the one to tick.

## 9. Tearing it all down

In this order. Each command is destructive; read it before running it.

```bash
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"

# 1. The service. This alone stops every possible charge from compute.
gcloud run services delete lbsim --project lbsim-gcp --region us-central1 --quiet

# 2. The domain mapping, if one was ever created (needs a verified-owner identity).
gcloud beta run domain-mappings delete --domain=lbsim.ai --region=us-central1 \
  --project lbsim-gcp --quiet

# 3. The images. List first; delete by exact digest or exact tag, never by prefix.
#    THIS ONE NEEDS YOU, not Claude: artifactregistry.writer can push but not delete. Tried, and it
#    returns IAM_PERMISSION_DENIED. Run it as yourself, or grant artifactregistry.repoAdmin first.
gcloud artifacts docker images list us-central1-docker.pkg.dev/lbsim-gcp/lbsim \
  --include-tags --project lbsim-gcp --quiet
gcloud artifacts docker images delete \
  us-central1-docker.pkg.dev/lbsim-gcp/lbsim/lbsim:<exact-tag> --project lbsim-gcp --quiet

# 4. The build source tarballs. This is the only thing here safe to remove wholesale, because
#    deploy.sh is the only writer of that prefix. Everything else in the bucket is run results.
gcloud storage rm gs://lbsim-gcp-runs/cloudbuild-source/** --quiet
```

One image is left behind that is not a deploy: the tag `wstest`, from verifying the container
against the workspace layout. It should be deleted, and only you can — see step 3.

Nothing prunes old images, so the registry grows by one image per deploy. The whole repository was
41 MB after four images, which at Artifact Registry's storage price is a fraction of a cent a month,
so this is housekeeping rather than cost. If it ever matters, an Artifact Registry cleanup policy is
the mechanism, and setting one needs `artifactregistry.repoAdmin`.

**Not part of tear-down, and not to be done by Claude:** deleting the Artifact Registry repository,
the results bucket, the service account, the `lbsim-gcp` project, or anything at all in `lbsim-prod`.
Those are yours. Filters used with any delete must be exact matches, never prefixes — a substring
filter is precisely how a budget was lost on 2026-09-06.

When this phase ends, revoke the deploy key:

```bash
gcloud iam service-accounts keys list \
  --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com
gcloud iam service-accounts keys delete <KEY_ID> \
  --iam-account=lbsim-deployer@lbsim-gcp.iam.gserviceaccount.com
```

## 10. What the deploy identity can and cannot do

Held: `run.admin`, `artifactregistry.writer`, `cloudbuild.builds.editor`, `iam.serviceAccountUser`,
`monitoring.viewer`, `serviceusage.serviceUsageConsumer`, and `storage.objectAdmin` scoped to
`gs://lbsim-gcp-runs`.

Deliberately absent, and none of it is wanted: any billing permission, any project-level IAM, any
quota change, `storage.buckets.*`, the ability to enable or disable an API, `monitoring.editor`, and
domain ownership. Everything above is designed to work inside that, or to say plainly that it cannot.

Two of those gaps have been hit for real, so they are known rather than theoretical:

* **`logging.viewer`.** Container logs and build logs cannot be read at all
  (`gcloud run services logs read` and `gcloud builds log` both return PERMISSION_DENIED for all log
  views). A build failure had to be diagnosed by inspecting the source tree instead of reading the
  error. This is the gap that will hurt first, and section 3 says so.
* **Deleting images.** `artifactregistry.writer` can push and cannot delete, so tear-down step 3 is
  yours. `artifactregistry.repoAdmin` would cover it, and it is not needed until then.

The consequence worth remembering: **Claude cannot see your budget.** Budget verification is yours
alone, and the instance caps in section 4 are the practical control — the budget is only the tripwire
behind them, and a budget alerts without stopping anything.
