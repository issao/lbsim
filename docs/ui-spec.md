# Dashboard specification

From Issao's specification, moved out of `docs/execution-plan.md` at his request and expanded
where the wire protocol constrains the design.

> Start building a stand-in dashboard for now just so that I can observe progress.

Section 5 is that stand-in: it is buildable today, against mock data, before any engine exists.

---

## 1. Surfaces

Three, as specified.

### 1.1 Homepage

A list of the interaction surfaces, with a one-line description of each and a link. Deliberately
plain. Its only job is to be the thing you land on and immediately know what you can do.

### 1.2 Load test dashboard

// Issao: Also a playback feature up top. Play, pause, change speed. rewind.

// Issao: Also a view that enables seeing a trace of sampled requests (sampled by latency bucket). That may have to be a popup that pauses the simulation.

The fully customisable playground. Two panels:

**Control panel, left. Tabbed.**

| Tab | Contains |
|---|---|
| Scenarios | preset scenarios, loaded with one click |
| Load | load-generation parameters, live tunable |
| Policies | policy selection and parameters, live tunable. Controls are generated from `PolicySpec`, which is now fully typed, so the panel cannot drift from what the engine accepts |
| Cluster | fleet shape: clusters, pools, replicas per pool, accelerator |
| Run | speed, pause, step, rewind, and the current simulated time |

**Observation suite, right. Tabbed.**

| Tab | Contains |
|---|---|
| Cluster health | replicas ready, warming, draining, ejected; failure events on a timeline |
| Service quality | time-to-first-token and inter-token latency percentiles, SLO attainment, goodput against throughput |
| Machine level | one row per replica, **paginated**; queue depth, resident KV, batch size, step time, prefix hit rate |
| Utilization | key-value cache utilization, memory-tier occupancy and bandwidth, wasted GPU fraction |

**"Live tunable" has a precise meaning here.** Some parameters change only the view and take effect
immediately. Others change physics, and the server must rewind to a snapshot and re-simulate.
`UpdateResponse.required_resimulation` says which happened, and **the UI must show it**, because a
chart that silently re-computed its own history while claiming to be live is worse than one that
pauses to say so.

### 1.3 A/B view

An alternative layout: the same load, two policies, side by side. Two runs started from the same
scenario and the **same seed**, differing only in the policy. The seed equality is not a nicety:
named independent random streams mean the workload does not shift when the policy changes, so a
visible difference is a difference in policy rather than in luck. The UI should refuse to place two
runs side by side if their seeds or workloads differ, rather than letting someone draw a conclusion
from two unlike runs.

### 1.4 Showcase

Cards, one per interesting dynamic, drawn from `docs/ARCHITECTURE.md` section 12. Clicking a card
starts a scripted walkthrough: the run advances, pauses at critical moments, shows a small popup
describing what is interesting, and offers a resume button.

**Walkthrough scripts are content, not interface.** A card is a JSON file in the frontend: a
scenario reference, and a list of steps each with a simulated timestamp, a body of text, and which
panels to highlight. The server needs no knowledge of any of it, which is why the scenario
catalogue and narration messages were removed from `ingress.proto` during the simplification pass.
Pausing uses `SetSpeed(paused: true)` and `StepForward` with a bounded duration, both of which
already exist.

---

## 2. How pagination works, now that the wire protocol has none

Issao removed pagination from `subscription.proto`, correctly: *"Page is all ui details, doesn't
belong here. Metric subscription is all based on selecting a cluster, a pool, a machine."*

So the machine-level view paginates **client-side over subscriptions**:

1. The client asks which replicas exist, from a cluster-scoped subscription.
2. It sorts locally by whatever column the user clicked.
3. For the twenty rows currently visible it opens twenty per-replica subscriptions.
4. Changing page closes those and opens twenty others.

Three things fall out of that, all good. Payload is bounded by what is on screen rather than by
fleet size, so a sixty-thousand-replica run costs the same to watch as a hundred-replica one.
Sorting is instant, because it is local. And an abandoned tab stops renewing its leases, which is
also the mechanism that lets a cloud instance scale to zero.

The one cost: sorting by a live value only sorts what the client is subscribed to. The honest
answer is to sort the coarse per-cluster summary to choose a candidate page, then subscribe. The UI
should say it is showing a page rather than implying a global ranking.

---

## 3. Data budget

Rules that keep the browser and the server both cheap:

- **Subscribe only to what is visible.** Switching tabs closes the previous tab's subscriptions.
- **Renew leases only while visible.** A hidden tab, via the page visibility API, stops renewing.
  This is the primary defence against a forgotten browser tab holding a cloud instance alive.
- **Sample rate follows the simulation speed the user chose**, expressed as points per simulated
  second. At high speed a chart wants fewer points per simulated second, not more.
- **Reconnect rather than hold a stream open indefinitely.** Streams are bounded server-side at
  fifteen minutes; the client reconnects transparently.

---

## 4. Technology

React with Vite and TypeScript. gRPC-web to `sim-ingress` through `tonic-web`, so no proxy is
needed in development or in production. Generated clients from `proto/`, so the frontend cannot
drift from the interfaces. Charts: a small library rather than a framework, since every panel here
is a line chart, a histogram, or a table.

No global state framework. The natural unit of state is a subscription, and subscriptions are
already keyed and leased.

---

## 5. The stand-in, buildable now

Issao asked for something to observe progress with. The engine does not exist, so the stand-in runs
against **mock data generated in the browser**, with the same shapes the real interfaces define.

What it gives, immediately: the layout can be criticised, the tab structure can be found wanting,
and the pagination story above can be proven or disproven before any of it is wired to a server.
Those are the expensive mistakes to make late.

What it deliberately does not do: pretend the numbers mean anything. Every panel carries a visible
"mock data" marker until it is wired to a real run. A dashboard that looks real while showing
invented numbers is how someone ends up trusting a chart that was never connected.

Scope:

- Homepage with the three surfaces.
- Load test dashboard: both panels, all tabs, mock time series that respond to the control panel in
  plausible ways.
- A/B view with two mock runs.
- Showcase with cards and one scripted walkthrough, driven by a real JSON script so the format gets
  exercised.

Then, when `sim-ingress` exists, the mock data source is replaced by a real client behind the same
interface, one panel at a time.
