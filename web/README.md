# lbsim stand-in dashboard

The three interaction surfaces from `docs/ui-spec.md`, running against **mock data generated in the
browser**. The engine does not exist yet; this exists so the layout, the tab structure and the
pagination story can be criticised before any of it is wired to a server, because those are the
expensive mistakes to make late.

Every panel carries a visible `mock` marker, and the header says `mock data, no engine attached`.
That is deliberate and should stay until each panel is actually wired.

## Running it

```
cd web
npm install
npm run dev        # http://localhost:5173
```

```
npm run build      # tsc --noEmit && vite build, output in web/dist
npm run preview    # serve the built output
npm run typecheck  # tsc --noEmit alone
```

Node 22, npm 10. Dependencies are react, react-dom, vite, typescript and the Vite React plugin, and
nothing else: no state framework, no component library, no CSS framework, no charting library. Every
chart is hand-written SVG in `src/components/charts/`, which for a line chart, a heatmap, a
histogram and a waterfall is less code than configuring a library would have been.

## The surfaces

| Route | What it is |
|---|---|
| `#/` | Homepage. A list of the surfaces, one line each. Deliberately plain. |
| `#/dashboard` | Load test dashboard. Playback across the top, control panel left (Scenarios, Load, Policies, Cluster, Run), observation suite right (Cluster health, Service quality, Machine level, Utilization, Traces). |
| `#/ab` | A/B view. Same load, same seed, two policies. Refuses to compare runs that differ in anything else. |
| `#/showcase` | One card per dynamic from `docs/ARCHITECTURE.md` section 12. Four have scripted walkthroughs. |

## What is mocked, and what is real

**Mocked: all of the numbers.** `src/lib/engine.ts` is a stateful loop over half-second ticks with a
snapshot every ten simulated seconds. It is not physically accurate and does not try to be. It is
arranged so the *directions* are right, because a layout can only be judged if the panels move the
way the real thing would:

- raising the arrival rate raises queue depth, then time-to-first-token, then loses goodput;
- round robin with a heavy tail of request sizes produces a hotspot that walks the fleet, and
  turning long-request probability to zero makes it disappear;
- least-KV-tokens with stale telemetry produces a travelling wave of herding, and
  power-of-two-choices under the same telemetry does not;
- parked sessions hold context between turns, so key-value capacity binds at request rates the
  fleet could otherwise serve, and the batch cap that follows is what turns a cache shortage into a
  spiral rather than a plateau.

**Real: the shapes, and the parts of the protocol a user can see.** These were built against
`proto/lbsim/v1` so that replacing the data source is mechanical rather than a redesign:

- metrics are the `Metric` enum by number (`src/lib/types.ts`);
- a subscription names exactly one entity, is leased, is renewed while visible and is closed on
  unmount (`src/lib/subscriptions.ts`); the status bar shows open, peak, opened, closed, expired and
  renewals, which is how you check that the data budget in spec section 3 actually holds;
- distributions are log-linear bucketed histograms merged bucket-wise (`src/lib/hist.ts`), so
  percentiles and SLO attainment are *derived at render time*;
- the responses that carry information the user is owed are surfaced rather than swallowed: where a
  `StepForward` stopped, whether a `Rewind` came `from_log`, and whether an update set
  `required_resimulation`.

Two consequences worth knowing, because they are the parts most likely to be judged wrong:

- **A physics change really does rewind.** Move anything in Load, Policies or Cluster and the run
  restores its last ten-second snapshot and re-simulates forward from there. The frames before that
  point keep the old parameters, so the chart visibly changes character at the boundary, and the
  banner names the boundary. Moving an SLO threshold instead recomputes attainment from the recorded
  histograms and says `required_resimulation = false`.
- **The scrubber is honest about its two halves.** The filled region is recorded and scrubbing
  inside it is a log read. The hatched region is not simulated yet and dragging into it re-simulates,
  which the caption says before you click rather than after.

## Replay mode: recorded runs from static files

The first dashboard showing real engine numbers, without a live server. `sim-run export` writes
each run as the exact documents the live stream carries (`crates/sim-ingress/src/export.rs`, WIRE.md):
`runs/index.json`, then per run `status.json`, `fleet.jsonl`, `result.json` and `scenario.txt`. When
no server is configured and `runs/index.json` is served beside the app, the load-test dashboard
plays those runs; when it is absent, nothing changes and the mock runs as before.

For local development, export the demo runs and place them under `public/runs/`, which Vite serves
at `/runs/`:

```
cd .. && tools/build.sh run --release --quiet --bin sim-run -- export --demos --dir /tmp/lbsim-export
rm -rf web/public/runs && cp -r /tmp/lbsim-export/runs web/public/runs
cd web && npm run dev        # http://localhost:5173/#/dashboard
```

**Do not commit `web/public/runs/`.** It is a 16 MB build product of the engine, regenerated by the
export; `git add` by explicit path is what keeps it out. In production the exporter writes into the
container's `--dir` and the static server serves it from the same origin, so the same relative fetch
works with nothing configured.

What you get, per `docs/dashboard-plan.md` section 3:

- The header says `replay of a recorded run: <run_id>`, and the banner above playback names the run,
  its routing, replica count, sample count and rate, seed and state checksum, with a picker over
  the index grouped by demo (`1-routing`, `2-staleness`, ...). `?run=<run_id>` on the URL picks the
  run for that tab; `?replay=0` forces mock for that tab so the two can be compared side by side.
- Load, throughput, latency (all four distributions), imbalance and KV are the engine's numbers
  (`src/lib/adapter.ts` maps a `SubscriptionUpdate` row to the panels' `Frame`, metric by metric as
  WIRE.md's table lists them). Distributions arrive as per-window exact percentiles and are rebuilt
  into the log-linear histograms the panels merge and derive from; the histogram's quantiles land
  within one bucket of the wire's, and the wire's own values are kept on the frame (`exact`,
  `latencyMs`) for anything that wants the measured p99 rather than the bucketed one. A window with
  no completions has no latency at all, NaN rather than zero.
- Quantities the engine does not simulate yet (preemptions, wasted GPU, prefix hits, memory tiers)
  are NaN: a gap in a chart, a dash in a tile. The lifecycle counts are 0 because the fleet is static,
  and the per-replica table and heatmap are empty until the per-replica export rows land. Every
  panel still carries its `mock` tag; removing them panel by panel is the next unit's job.
- Play, pause, speed, step and scrub work locally over the loaded frames, so every scrub is a log
  read. Rewind-and-resimulate, workload and policy changes and restart are refused with the reason
  `replay of a recorded run; changing load or policy needs a live engine`, and the banner names the
  change that was not applied. SLO thresholds and the sample rate still apply, because they are
  view-only: attainment is re-derived from the recorded distributions.
- The status bar says `mode replay` and where the frames came from.

The A/B view and the showcase stay mock: the showcase scripts drive the mock's dynamics, and two
recorded runs side by side is a small follow-up on the same source.

The replay path has its own self-test, run the same way as the transport's and with the same
harness style:

```
cd web && node --experimental-strip-types src/lib/replay.selftest.ts
```

It ends with `15 cases, 15 passed, 0 failed`. The fixtures are two real lines of the p2c demo's
`fleet.jsonl`, its index entries and its `scenario.txt`, so the numbers checked are the engine's.
The file registers a small Node resolve hook for extensionless `./module` imports before loading
anything, because adapter.ts and replay.ts import `./api` and `./hist` the way the rest of the app
does and Node's loader, unlike Vite's, wants the extension.

## Walkthrough scripts

Content, not interface. `public/walkthroughs/index.json` lists one card per dynamic;
`public/walkthroughs/*.json` are the scripts, fetched at runtime so the format is genuinely
exercised. `public/walkthroughs/schema.md` documents it. Five are written:
`rolling-hotspot` (dynamic 1, seven steps, the complete one), `stale-telemetry` (2), `kv-spiral` (4),
`affinity-vs-spread` (8) and `gray-failure` (10). Cards without a script are listed and disabled, so
the catalogue shows what does not exist yet rather than quietly omitting it.

## Server mode

Mock stays the default. The transport to the real Ingress server lives in `src/lib/api.ts`
(messages, the HTTP layer, the SSE reader, the subscription lifecycle), `src/lib/useServerRun.ts`
(the `useRun` seam backed by the server) and `src/lib/mode.ts` (the flag). In mock mode none of it
executes, so the stand-in is unaffected.

The wire it speaks is `crates/sim-ingress/WIRE.md`, which is the authority: `POST
/v1/ingress/<RpcName>` with JSON bodies for the unary RPCs, `GET /v1/ingress/OpenSubscription?<query>`
for the metric stream as server-sent events (`event: open` then `event: update`, each update with
an `id:`), proto field names verbatim in snake_case, every `uint64` a decimal string on the wire and a
`bigint` in TypeScript, enums by name, enum-keyed maps by number. On a dropped stream the client
resumes with `Last-Event-ID`; a `410 Gone` makes it resubscribe from scratch. The lease is counted
down locally from `lease_ns` and the renew response's `expired` flag is the only authority; the
server's `lease_expires_at_wall_ns` is never compared to the browser clock.

Turning it on, in precedence order:

| How | Effect |
|---|---|
| `?server=1` on the URL, or `#/dashboard?server=1` | this tab, same origin |
| `?server=http://localhost:8099` | this tab, that server |
| `?server=0` | this tab back to mock, whatever else is set |
| `localStorage.setItem('lbsim.server', '1')` (or a URL, or `'0'`) | this browser, until removed |
| `VITE_LBSIM_SERVER=1` (or a URL) at build time | the build's default |

The header banner says which: `mock data, no engine attached` or `live data from the Ingress
server at <base>`.

`src/lib/config.ts`'s `BASE` is `scenarios/base.txt` key for key, so a run started from the default
control panel is the run `sim-run run scenarios/base.txt` performs; the self-test reads the file and
checks. A `StartRun` sends the whole config as the flat `key = value` text WIRE.md describes, with an
empty `overrides` map; `UpdateWorkload` and `UpdatePolicies` send only their own keys as string
overrides.

The transport has a runnable self-test with no network, no browser and no test framework: it runs
on the fixtures in `src/lib/apiFixtures.ts` (including WIRE.md's own examples verbatim), a stub
`fetch`, and two files read from the repo, the protos and `scenarios/base.txt`.

```
cd web && node --experimental-strip-types src/lib/api.selftest.ts
```

Node 22's built-in type stripping runs it; nothing is installed for it. It prints one `ok` line per
case and ends with `33 cases, 33 passed, 0 failed`, or throws so the exit code is the answer. It
also checks that every wire field name the client reads or writes exists in `proto/lbsim/v1`, which
is the client-side half of the guard WIRE.md describes for the server.

## What must change when `sim-ingress` exists

One panel at a time, behind the same interface.

1. **Generate the clients.** Replace `src/lib/types.ts` with generated code from `proto/`, so the
   frontend cannot drift from the interfaces. The names here are already the proto names in
   camelCase; the enum values are already the proto numbers.
2. **Replace the run controller.** `src/lib/useRun.ts` is the seam. Its methods map one to one:
   `setPaused`/`setSpeed` to `SetSpeed`, `step` to `StepForward`, `rewindTo`/`scrubTo` to `Rewind`,
   `update` to `UpdateWorkload` plus `UpdatePolicies`. Keep the return values: the UI already shows
   them, and dropping them would remove the honesty the spec is asking for.
3. **Replace the subscription registry.** `src/lib/subscriptions.ts` becomes gRPC-web
   `OpenSubscription` / `RenewSubscription` / `CloseSubscription` over `tonic-web`. The lifecycle the
   panels rely on -- open on mount, renew while visible, close on unmount -- does not change, and
   `useSubscriptions.ts` should not need editing. Add the fifteen-minute reconnect, which the mock
   has no reason to model.
4. **Replace the frame store.** Panels read `Frame` objects from `MockEngine.window()`. Real
   `SubscriptionUpdate` messages arrive per target rather than per fleet-wide frame, so this is the
   one place with real work: a store keyed by `(target, metric)` that panels select from. Doing this
   properly is what proves or disproves the pagination story in spec section 2.
5. **Delete the mock.** `src/lib/engine.ts`, `src/lib/traces.ts` and `src/lib/rng.ts` go, along with
   `MockTag`, the header badge, and the "no network" note in the status bar. Nothing else should
   need to change, and if it does, that is a design problem worth knowing about.

Two things this stand-in does that the real one must not: it computes fleet percentiles by mixing
per-replica lognormals rather than measuring requests, and it treats the three SLO targets as
independent when computing attainment. Both are marked in the code.

## Layout notes

Dense and instrument-like rather than consumer: 12 px base, tabular figures for every number, one
accent, four reserved status colours, three series hues, and a single sequential blue ramp for
magnitude. Light and dark are both selected -- the dark steps are chosen against the dark surface,
not flipped -- and follow `prefers-color-scheme`. No emoji, no icon font, no images.
