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

## Walkthrough scripts

Content, not interface. `public/walkthroughs/index.json` lists one card per dynamic;
`public/walkthroughs/*.json` are the scripts, fetched at runtime so the format is genuinely
exercised. `public/walkthroughs/schema.md` documents it. Five are written:
`rolling-hotspot` (dynamic 1, seven steps, the complete one), `stale-telemetry` (2), `kv-spiral` (4),
`affinity-vs-spread` (8) and `gray-failure` (10). Cards without a script are listed and disabled, so
the catalogue shows what does not exist yet rather than quietly omitting it.

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
