# Execution graph

The tech lead's dependency graph of every unit of work: done, in flight, queued, or waiting on Issao.
Owned by the tech lead; updated on every spawn, merge and ETA change, in the same commit where possible.
Per Issao: *"keep an instruction graph of everything that we need to in an md file, with sections below
of what each task entails."* A stale graph is worse than none, so the status line moves every time.

**Last updated:** 2026-09-07 21:54 PDT. **RUNNING** (tech lead resumed 21:00 PDT on main's brief: Issao's three asks, machines page shows 0 replicas / slider to 10k / default 256 replicas, plus main's U94 GPU utilization and U95 partial-tag wording). Deployed: lbsim-00016-nbc from 3de8220; master is 6a7453b (proto: METRIC_GPU_UTILIZATION 67, METRIC_GPU_COMPUTE_BOUND_FRACTION 68). Housekeeping and productivity are not running; landings are recorded here only. **Spawned 21:00:** U90 (default, `claude/tl-machines-live`, port 8195), U91 (sonnet, `claude/tl-replicas-10k`, port 8196), U92 (default, `claude/tl-default-256`), U93 (sonnet, `claude/tl-qa-replay-wait`, port 8199), U94a (default, `claude/tl-gpu-engine`), U94c (default, `claude/tl-gpu-web`, port 8197), U95 (sonnet, `claude/tl-partial-tag`, port 8198). U94b (wire) spawns when U94a lands. **21:07:** U96 (default, `claude/tl-ab-live`, port 8200) spawned from main's A/B decision; U108 (lockstep A/B via StepForward) queued. **21:09:** master was red since 6a7453b (the proto's two GPU metrics were missing from `wire::METRICS`, so `metric_table_matches_the_proto_enum` refused every integration at stage 3); the tech lead's one-line unblock fix is fc6d310. U93 is done on its branch (6ce6f2a: lbsim.ai 57/2 before, 59/0 after) and re-integrating. **21:12:** U94a landed (20cb4d9); U94b (sonnet, `claude/tl-gpu-wire`) spawned. **21:13:** U93 landed (fb9972b), U90 landed (258c706), U91 landed (c9d7c0d). In flight: U92, U94b, U94c, U95, U96. **21:18:** U92 landed (ba009c7); before/after numbers per finding sent to main (finding 2, staleness, needs rewriting: at 256 replicas the herd is past the cliff at 100 ms). U94c is re-integrating after a two-line MachineLevel fix (U90's `pendingRow` lacked the new fields) with ownership of types.ts extended. In flight: U94b, U94c, U95, U96. **21:20:** U94c landed (64d9ad0), U95 landed (d01f991). Main's decisions on U92's numbers: demo 2 goes back to 32 replicas (**U92b**, sonnet, `claude/tl-staleness-32`, spawned 21:20); the cliff moving with fleet size is a finding of its own (**U98** queued, demo 13); a docs agent of main's rewrites findings.md. Main's load test on the release build: 10k replicas is 1.5–1.9× realtime on one core and the 120 s run trips `MAX_EVENTS` (**U97**, sonnet, `claude/tl-event-ceiling`, spawned 21:20); numbers recorded under M9 below. In flight: U94b, U96, U97, U92b. **21:21:** U96 landed (53c5993). In flight: U94b, U97, U92b. **21:32:** U92b landed (ff4e0ca: demo 2's rows byte-identical to pre-U92). U97 landed (2eb5d4f) with its ORIGINAL scope (scaled event ceiling + hardcoded 8 GiB RSS guard): the agent declined a mid-flight redirect relayed as a peer message, correctly, so Issao's design is **U97b** (sonnet, `claude/tl-memory-budget`, spawned 21:32): no event ceiling at all, `LBSIM_MEMORY_BUDGET_MB` (default 20000; the Dockerfile sets ~80 % of the container), in-flight/records/queue caps scaled with replicas × duration. **U99** (sonnet, `claude/tl-no-jump`, port 8203, spawned 21:32) from Issao's jumpy-cards note. **Standing rule from Issao (21:32): two dynamics lanes are always in flight**, working through docs/scope-today.md beyond package B in the graph's order (prefix caching 9/10, tiering 14, disaggregation 15, autoscaling 12, multi-geo 13, weight locality 17, control analysis M7, failover cascades); each lane ends its unit the way the demos do (scenario, report, recording, walkthrough, finding paragraph to main) and starts the next; dashboard/polish units run beside them. Lanes now: **lane A = U98** (default, `claude/tl-herd-fleet`, port 8201, demo 13), **lane B = U27a** (default, `claude/tl-prefix-engine`) with **U27b** (default, `claude/tl-prefix-policy`, port 8202, demo 14) coding against U27a's seam in parallel; U27c (wire + report + web keys) queued behind U27b. **U29 (Leaf shards as processes, M3) is future work by Issao's decision** ("let's not do sharding then, leave it recorded for future work"): the single-threaded core does 10k replicas at 1.5–1.9× on one core and the 5×10k-GPU target is 6,250 replicas, so sharding is the lever only past that; the U15 Leaf seam stays as the boundary it would plug into. In flight: U94b, U97b, U99, U98, U27a, U27b. **21:35:** Issao asked again what is mock about a live run → **U95b** (default, `claude/tl-no-invented`, port 8204): no invented number on live/replay, unwired cells render "—" with the hover "not simulated yet", tags carry the mode word only; **U101** (sonnet, `claude/tl-replica-state`): replica state, true speed and a per-replica TTFT mean on the wire (proto 69/70 landed by main at 21de1ba). Then Issao, verbatim: *"yeah, let's remove all invented numbers everywhere, every reference to mock in the ui, every link for a mock data view. it is not needed anymore."* → **U100** (default, web) queued to spawn when U95b and U99 land so the last web unit of the wave rebases the others rather than the reverse: the browser mock engine and everything that exists only for it goes; data modes are live and replay only; **the stand-in dashboard's role ended today.** deploy.sh already sets `LBSIM_MEMORY_BUDGET_MB=1600` for the 2 GiB container (661cc65). In flight: U94b, U97b, U99, U98, U27a, U27b, U95b, U101. **21:35:** **U102** (play button on the walkthrough card) queued behind U100. **21:35:** U94b landed (9d4aa1b): GPU utilization and the KV distribution are on the wire and in the export; only `summary_md5` moved. U94 is complete end to end; qa.js's two non-zero-mean checks should now pass (verified in the wave's final harness run). In flight: U97b, U99, U98, U27a, U27b, U95b, U101. **21:38:** **U103** (draggable walkthrough card) queued with U102. **21:39:** from main's vision-progress refresh: **U104** (real traces on the Traces tab, after U100) and **U105** (rewind from the checkpoint, after the lanes' current units) queued; **U31b** (outlier ejection / failover) is lane A's next unit after U98, because it unlocks three later items and needs nothing. Order of the remaining web units: U95b, U99 → U100 → U102, U103, U104. **21:45:** master was red a second time (main's d32f079 added 69/70 to the proto without the wire table); the tech lead's one-line fix is 2ba17f2. Done on their branches and re-integrating: **U98** (59994da: demo 13's five points, SLO 79.5 → 56.5 → 29.5 → 14.5 → 7.0 % from 32 to 512 replicas at 0.30 ratio and 250 ms; finding 10 paragraph in its report, forwarded to main with the READY), **U27a** (d6e1978: the seam as specified, `PrefixTree` lives in sim-workload and is re-exported by sim-model), **U97b** (b867135: no event ceiling, `LBSIM_MEMORY_BUDGET_MB` default 20000, caps scaled; the 10k-replica 120 s run completes locally in 71.5 s wall at 916 MB peak RSS). Both U98 and U27a edited `tests/scenario_parse.rs`'s exhaustive literal and KEYS count, so the second to integrate resolves that conflict. **21:48:** U95b landed (e88b73c); U100 spawns when U99 lands. Issao: *"where do i tune step token budget?"* → **U106** (editable physics knobs with a Restart path) queued after U100. **21:51:** U98 landed (d94fbf0), lane A moves to **U31b** (default, `claude/tl-ejection`, port 8205, demo 15). In flight: U99, U27a (re-integrating), U27b, U97b (re-integrating), U101, U31b. **21:54:** U27a landed (361d59a; U27b told to rebase); U97b landed (925f758: no event ceiling, `LBSIM_MEMORY_BUDGET_MB`, scaled caps; the 10k-replica 120 s run completes in 71.5 s wall at 916 MB peak). Issao: *"the scrolling of control seems wrong ... In general the full screen shouldn't scroll, just individual panels."* → **U107** (default, `claude/tl-app-shell`, port 8206) spawned beside U99. /tmp (16 GB tmpfs) filled to 97 % with agents' export scratch and made one integrate report a false "fingerprints moved"; cleaned to 45 %. In flight: U99, U27b, U101, U31b, U107. READY TO DEPLOY goes to main when U90–U96 are all on master and tools/qa/qa.js is green locally. **Done 92 · in flight 5 · queued 20 · waiting on Issao 1 · future work 1 (U29).**
**20:24, the stop:** U87+U88 landed (03bd74e: b598c0d plain facts in place of `UpdateWorkload`/`LoadShape`, `PolicySpec`, `required_resimulation` and the `bench/` path, the status bar's subscription sentence into a hover; bba5796 `Slider` gains `readonly` and the SLO targets and sample rate are readouts). Main deployed **lbsim-00016-nbc** from 3de8220 at 20:23 without waiting for the READY, since all four follow-ups were on master, and is running the harness three times against it; main records that result. Tonight, in order: U80 023672d, U82 837910b, U81 bd5ba3f, U79 60327b7, U77 f4a119f, U83 b5f5af7, U86 9c22131, U84+U85 90e6b93, U89 a9252da, U87+U88 03bd74e — ten units through integrate.sh, three deploys (00014 U79–U82, 00015 U83+U77, 00016 the follow-ups), the Cloud Run degradation fixed and proven (six harness passes, zero busy), the first-sample race proven and fixed. **Next tech lead starts from the queue below** (U25b, U31b, S3 simplification over sim-ingress which grew by U79 and U77, U69, then the engine roadmap), and from the harness result main records for 00016. Brief lessons for the productivity agent: run `npm run build` on a resumed WIP branch before spawning (U81 lost a round trip to a type its own checkpoint had widened); cite symbols with line numbers (three units reported drift); a grep in a brief must recurse (U89's pager was one directory below); a resumed unit's brief should carry the sibling's file ranges when two units share a file (U84+U85 and U87+U88 landed without a conflict because each brief named the other's range).
**20:10:** U89 landed (a9252da: `.card-needs` with `min-width:0` and ellipsis, and `.card-foot` itself needed `min-width:0; width:100%` because as a flex item of the column card it grew to its content and spilled past the border, measured 534 px against a 334 px card; the "Prove the refusal" block and its `drift` state gone from Compare.tsx; the replica table's row-size picker hidden at zero rows and prev/next hidden at ≤10 rows; the "no A/B view yet" copy replaced; one flaky `ERR_CONNECTION_REFUSED` run, clean on rerun, 44 PASS). Brief gap: the pager lives in `web/src/panels/observe/MachineLevel.tsx`, one directory below the grep the brief gave. Remaining in flight: U87+U88.
**20:01:** U84+U85 landed (90e6b93: `ControlPanel` passes `data={{kind:'real'}}` so the header tag is the page's word; a `ReplayLocked` fieldset around the load, policies and cluster tab bodies on replay, tagged "recording" with the refusal reason as its title, done at the `ControlPanel` level so the sibling's tab bodies were never touched; three qa.js checks; 47 PASS locally). Remaining in flight: U87+U88, U89.
**20:01:** U86 landed (9c22131). The hypothesis held: self-test case (l) issues `setSpeed(2)`/`setPaused(false)` the moment `runId` appears against a fake that applies concurrent SetSpeeds in reverse order; on master the run ended paused, with the engine's per-run control chain (`controls` promise, `control()` helper; `setPaused`/`setSpeed`/`step` and `start()`'s own post-StartRun call queue on it, each reading the latest wish; `runId` published only after queuing) it plays. Part 2 was a real leak: `start()` never re-checked `generation` after its SetSpeed await, so a dispose there was followed by `subscribe()` and a `setInterval`, hidden by `disposed` until the next `start()` reset it and overwrote `this.poll`; case (m) counted GetRun 2→4 on master. Fixed: re-check after the await, clear any interval before setting one; the harness also keeps one card's page open for section e, which explains part of main's r-77 sighting. `mode=""` was the element absent: `.wt-mode` sits in `overlay`, which `DashboardBody` rendered only with a frame; now in the no-sample branch too. 13 self-test cases, 44 PASS locally. Not done: no dedupe of identical consecutive SetSpeeds.
**19:55:** main deployed **lbsim-00015-79v** from b5f5af7 at 19:51 (U83 and U77 on top of 00014's U79–U82: the whole morning instruction is on lbsim.ai). Main, on the two U83 leftovers: they are inside Issao's instruction, so spawn now rather than queue, with the reviewer's findings folded in where small; main calls that unit "U87", which on this graph is **U84+U85 as one unit** (sonnet, `claude/tl-panel-mode`, port 8198): the Control panel's header tag follows the mode via `data={{kind:'real'}}`, and on replay the load, policies and cluster tabs render inside a disabled fieldset with the one-word reason "recording"; qa.js asserts both. The reviewer's findings spawned beside it as **U87+U88** (sonnet, `claude/tl-panel-text`, port 8199: internal names out of the tab bodies and the status bar; `Slider` gains `readonly` and the SLO targets and sample rate render as readouts) and **U89** (sonnet, `claude/tl-showcase-tidy`, port 8200: card-foot ellipsis, the "no A/B view yet" copy, the empty replica pager, the A/B "prove the refusal" block). File overlap noted on each brief: U84+U85 owns `ControlPanel()` (lines 20–58), U87+U88 the tab bodies, U89 stays out of U86's Showcase range; a stage-2 conflict between siblings is theirs to resolve. One READY when U86, U84+U85, U87+U88 and U89 are on master; main deploys and runs the harness three times; then the tech lead stops.
**19:52:** the U83 reviewer (sonnet) walked the eleven after-PNGs: the verbose live header, the duplicate Playback block and the `mock-base-…` run id under a live badge are gone; eleven findings, queued under main's stop rule as three sonnet units: **U87** internal names out of the control panel and status bar (Load tab foot "UpdateWorkload replaces the whole LoadShape", clipped by the footer; "PolicySpec has no fields for it"; "the banner above says `required_resimulation` = false"; "step_base_ms is calibrated in `bench/validate_epochs.py`"; the status bar's "subscription counts on the left are this page's own" line becomes a hover); **U88** the view-only SLO-target and sample-rate sliders render as readouts, not draggable handles; **U89** Showcase `needs:` text truncates mid-word without an ellipsis, the walkthrough copy "this page has no A/B view yet", the replica table's pager showing over "0 replicas", and the A/B page's "prove the refusal" checkbox (a test affordance) goes. Not taken: hiding the unscripted cards (the spec lists them so the catalogue's shape is visible) and removing the greyed perturbation select on live (U83's `dropped` wrapper doing what it should).
**19:51:** main accepted U79 on lbsim-00014-gvp: its three harness passes 54/2, 55/1, 56/0, zero `busy` across ~90 runs, end reasons `final`×4, `lease expired`×6, `no frames`×1; the degradation is gone. The tech lead's three: 55/1, 56/0, 54/2. Every failure is one showcase card stuck at 0 samples with its run streaming — six passes, six different cards (deadline_aware, past-the-knee, gray-failure, rolling-hotspot, stale-telemetry, fair-share), about one in fourteen, never locally. Main asked for one small unit on that and on a `GetRun` poller outliving its page (r-77 polled while r-86/r-87 ran). **U86 spawned** (default, `claude/tl-first-sample`, port 8197): hypothesis to prove with a failing-then-passing self-test: the Showcase mounts its dashboard with `autoplay=false`, so `start()` sends `SetSpeed(paused=true)` after StartRun while the runner, created as soon as the run id is published, sends `play()`'s `SetSpeed(paused=false)` concurrently; arrival order decides, and a pause landing last leaves the run still with its stream open. Fix: one promise chain per run for controls, the run id published after `start()`'s own SetSpeed is queued. Part 2: `start()` runs on past `dispose()` after its SetSpeed await; re-check the generation after every await, clear a leaked interval. Part 3: the walkthrough header's mode word from first paint (the harness reads `mode=""` on stuck cards), qa.js waits up to 20 s for the first sample. READY for U86 when it lands, then three remote runs, then stop.
**19:49:** U83 landed (b5f5af7: rewind buttons absent off-mock, RPC-name titles to words, the `mock run` tag gone, legend reads "not yet reached" off-mock and is dropped once fully recorded; Run tab's Playback section gone, run id from `run.source.runId`; live and replay banners one line with the long reason in `title`; a generic `<Dropped path>` wrapper greys all six server-dropped paths with "server" (perturbation block, accelerator, `routing.maxLoadRatio`, `routing.fallbackChoices`); `tools/qa/screens.js` plus a two-line `QA_SCRIPT` knob in serve-local.sh; before/after PNGs under `/tmp/lbsim-screens-before/` and `/tmp/lbsim-screens/`; local gate 44 PASS / 12 report-404s). **All five of the morning's units (U79–U83) and U77 are on master; READY TO DEPLOY b5f5af7 sent to main.** Remote harness on lbsim-00014-gvp, run 1: 55 passed / 1 failed, the first showcase card stuck at 0 samples after 10 s with its run streaming, every later card green, server log clean (no busy, no non-200); runs 2–3 running. Queued from U83's report, not spawned under main's stop rule: **U84** (sonnet) the Control panel header's `MOCK` tag shows in every mode, including live, where changes go to the server (`Panel` in ui.tsx); **U85** (decision) on replay the load and policy tabs look active although every change is refused. A sonnet reviewer is walking the eleven after-PNGs; findings join the queue. Brief lessons: cite symbols with line numbers (MockTag was at 158, not 165); `dropped` had six paths, not two; `RunHandle.source` is optional.
**19:41:** main deployed **lbsim-00014-gvp** from c2329f7 (carries U79–U82) at 19:39; the tech lead is running `QA_BASE=https://lbsim.ai node tools/qa/qa.js` against it. U77 landed (f4a119f: `RunState.{idle_stopped_at_wall_ns, evicted, started_at_wall_ns}`; `start()` evicts the oldest idle-stopped run before answering 503; `drive()` exits on `evicted` and reaps itself after 2 × threshold through a `Next::Reap` that unregisters after the state lock is released; `Registry::remove` takes `runs` alone; WIRE.md paragraph; 52 tests, the two timing tests 10/10 repeats; no 410, no server.rs change). Remaining in flight: U83.
**19:36:** U79 landed (60327b7: `stream_updates` decides a `Step` under the `subs` lock and acts after the guard drops, `finish_subscription` never called with a guard held, end reasons `final` / `no frames` / `lease expired` / `superseded` / `closed` / `write error`; `drive()` reads `live_for_run` before `run.lock()`; lock order leases → subs → run.state commented at both accessors and in WIRE.md; the two WIP tests hung 5 s on the unfixed code and pass now, a third proves a second OpenSubscription still opens after a stop; `GET /requests.log`, 512-line ring, `busy 503 connections=N` on shed; 50 crate tests). Harness: 38/12, 38/12 (report 404s under `QA_SKIP_REPORTS=1`), then 50/0 with reports; run 2's log: 718 lines, `lease expired`×15, `final`×1, zero busy. Not done, small: rejected opens (400/404/410) are not `req` lines. **READY TO DEPLOY 60327b7 sent to main** with that output, ahead of U83, because the hang is the live-site fault; a second READY follows U83. **U77 spawned** (default, `claude/tl-run-cap`): count busy-or-leased only, evict the oldest idle-stopped run at the cap (`evicted` flag, thread exits, checkpoint stays, GetRun 404), reap after 2 × IDLE_SHUTDOWN_SECONDS from the run thread; two tests on a 50 ms threshold.
**19:35:** U81 landed (bd5ba3f: `badgeText`/`badgeTitle` rendered in App.tsx, `none` on every hash change, `connecting`/`refused`/`server` from the dashboards, `mock` from Compare, brand line "inference fleet simulator", qa.js asserts the badge per route and Home's three headings, card filter `!e.disabled`; 44 PASS locally, the 12 FAILs are the report 404s under `QA_SKIP_REPORTS=1`). It found and fixed a real race its own assertion caught: React runs mount effects child-first, so App's `setActiveMode('none')` in a `useEffect` clobbered Compare's `mock`; App's initial reset is a `useLayoutEffect` now. **U83 spawned** (default, `claude/tl-ui-pass`, port 8195): playback bar reads play / speed / position with rewind absent where rewind is off, the "mock run" tag gone, the Run tab's duplicate playback controls gone, live and replay banners one line each with the long reasons on hover, `dropped` controls disabled in place, `tools/qa/screens.js` screenshots every route and tab before and after. U80–U82 on master; READY TO DEPLOY goes to main when U83 lands.
**19:25:** U82 landed (837910b: index.html title `lbsim`; Showcase intro without repo paths or the schema link, card foot `coming`; StatusBar live line; index.json note and the four `requires`; least-kv-probe:48 and retry-storm:34; no build word in `out/*.html`). Its local gate: the one non-report FAIL was the known one, the card filter on the phrase it removed; U81 lands `!e.disabled`. U81 was blocked for one line outside its files: the pause checkpoint's `mode.ts` widened `activeMode().mode` to `ActiveModeName`, which broke `ui.tsx:59` (`panelTagWord` takes `DataMode`); granted that one line (none/connecting/refused map to `mock`) and it is finishing the gate. Brief lesson: a resumed WIP commit can have moved a type that callers outside the file list depend on; `npm run build` before spawning would have shown it. U82 reported the brief complete.
**Previous entry, kept for the history:** PAUSED 2026-09-07 11:24 PDT, quota; Issao, verbatim: *"actually, pause everything, we are almost out of quota. we will resume when we are back within limit"*. Every wave-1 agent was told to commit a "WIP: <unit> pause checkpoint" by explicit path, push its branch and stop; none ran integrate.sh. No monitor of the tech lead's is running. Resume paragraphs are in each unit's section below; the Resume order is U79, U81, U80, U82, then spawn U83 once U81 is on master. Deployed: lbsim-00013-mvq from master c22ea56. Housekeeping is running (agent a33bf6bd10b64d29f, `claude/docs-round38`); productivity is not. **Session goal, Issao this morning, verbatim:** *"is the load test dashboard link ready to point to the live sim engine? can you make sure that all links in the homepage are separated by a section for 'live' one for 'replay' and one for 'mock' and that the message in the top right corner for each page is accurate. also, clean up all text from any reference of the build process to make this look like a finished product, ensuring accuracy and succinctness. also take a pass over all the ui to keep things clean and simple and intuitive, with no knobs that are not doing anything."* Five units, U79–U83, briefs below; the gate is `tools/qa/qa.js` locally and then `QA_BASE=https://lbsim.ai` after main redeploys. **Done 67 · in flight 4 · queued 12 · waiting on Issao 1.**
**11:19, wave 1 spawned:** **U79** (default, `claude/tl-cloud-hang`, port 8191) the Cloud Run degradation plus U77; **U80** (sonnet, `claude/tl-home-sections`, 8192) Home in three sections; **U81** (sonnet, `claude/tl-mode-badge`, 8193) the top-right badge accurate per route and state, asserted by the harness; **U82** (sonnet, `claude/tl-finished-text`, 8194) build-process references out of user-facing text. **U83** (default, the UI pass) waits for U81 because both edit `Dashboard.tsx` in adjacent hunks. Read while briefing, recorded here: (1) main's brief puts the A/B page under "Replay"; `Compare.tsx` runs `useRun` twice, the mock engine on both sides, so it is **mock** and Home lists it there (told main). (2) The likely cause of the Cloud Run hang is in `server.rs::stream_updates`: two `return self.finish_subscription(id)` sit inside the block that holds the `subs` guard, and `finish_subscription` locks `subs` again on the same thread, which on Linux parks the thread forever holding `subs`; the next request whose `reap()` finds an expired lease then blocks on `subs` with no response (StartRun after a 60 s lapse is the usual victim, and every later request with nothing expired still answers, which is exactly the "GetRun answers in 120 ms" observation), every new OpenSubscription blocks at `self.subs().insert` after the lease is opened and before `sse_head` (no bytes), and each stuck thread keeps a connection slot until the 64 cap says "busy". The triggers are a run that stops, fails or finishes before its first closed frame (`n == 0 && ending`, a tab closed within the first sample interval, likelier on Cloud Run where the run thread has CPU only during a request) and a reconnect that already holds the final sequence (`sub.finished`). A second, latent fault: `drive()` takes `reg.leases()` while holding `run.lock()` (run.rs ~402), the reverse of `stream_updates` (`subs` → `run.state`) and `reap` (`leases` → `subs`); the order to document is leases → subs → run.state. U79 is briefed to prove both with tests before fixing.
**21:32:** U74 landed (a5061da: `start()` sends `maxRealtimeFactor: lastFactor` on both paths, `speed` falls back to `lastFactor`, `speedLabel()` in the banner; local browser check: slider 9.5 → 13.5 over 4 s at `speed 1×`; no existing case had asserted 0 on the playing path). Remaining in flight: U73, U75, U72, U71.
**21:36:** U72 landed (77fb48f: `ScenarioConfig.extra`, `EXTRA_KEYS` of 25 including `disable_decode` and `swap_gbps` which the brief missed, `diffConfig` walks both sides' keys, an unaccepted key throws by name; kv-spiral.json is kv_spiral_never.txt key for key; Playwright saw the card's StartRun carry `preemption = never`, `session_turns_mean = 8`, `session_think_s = 8`). Main fixed the Dockerfile itself (9c754c8: reports 11 and 12 built), so U71's links resolve after the redeploy. Remaining in flight: U73, U75, U71.
**21:38:** U73 landed (8c75b9f: `tools/qa/qa.js`, `serve-local.sh`, playwright-core 1.47.0 pinned; local gate 14 passed / 29 failed on the pre-wave master, lbsim.ai 20 / 23) and U71 (f97c175: assertions against the exported banner constants, `.wt-mode`/`.tag-line`, Home lists twelve reports). The gate's first run found two things outside main's five: **every showcase card is stuck at "0 samples" on the live path**, locally and on lbsim.ai, because the walkthrough runner's `setSpeed(2)`/`play()` fire from the dashboard's first render while StartRun is in flight, `setPaused` returns on the null run id, and `start(play=false)` then pauses the run once the id arrives with nothing left to unpause it; and **runs outlive closed pages** (no unmount on `page.close()` or a closed tab, so no StopRun) until the server's eight-live-run cap answers 503 for everyone. **U76 spawned** (`claude/tl-pending-controls`, default, port 8186): controls issued before the run id are remembered and applied once after StartRun; a `pagehide` listener stops the run with a keepalive StopRun; the harness stops every run it started. Decision for main, not made here: an idle-stopped run still counts toward the 503 cap of eight, so eight abandoned tabs lock the public site until their leases lapse; the server should either exclude idle-stopped runs from the cap or reap them. Remaining in flight: U75, U76.
**21:39:** U75 landed (de7536f: `#/showcase?script=<id>`, `App.currentRoute` strips the query, `probeDataSource()` exported and used by the Showcase so the narration word matches the run; 10/10 browser checks, 5/10 on master before). Answer to main's item 3b: only a hash carrying `?script=<known id>` opens a run at `#/showcase` without a click, which is the designed URL; no storage flag, no index default, no stale EventSource. U75 also met the null-run-id race and worked around it in Showcase.tsx (the runner is created once `run.source.runId` exists); U76 fixes the root in the engine, and the two compose. Remaining in flight: U76.
**21:50:** U76 landed (945a727: `wantPaused` + `pendingControl`, one SetSpeed after StartRun only when a control arrived while no run id existed, none otherwise; `pagehide` → `dispose()` with a keepalive StopRun beside the client call; qa.js stops every run it started and asserts none remain running: "13 started, 0 still running"; 11/11 server self-test). Deviation it found and made: the runner's controls fire *before* `start()` is called, not merely before it answers, because `DashboardBody`'s `onRun` is a child effect; `start()` therefore honours a control recorded earlier rather than resetting to `!play`. Its gate run predates U75 (wording and nav lines were still red); the full gate on master 945a727, reports included, is running now on port 8190. Two things it noted for the record: the `least_kv_probe` card's StartRun is refused 400 by the server (to be read off the gate output), and `serve-local.sh` needs `NODE_PATH=/home/agents/.local/opt/node/lib/node_modules` where playwright-core is a global install.
**21:55, the gate on master 945a727 (fresh build, reports included): 48 passed, 2 failed.** Items 1–4 are green in the browser: the dashboard opens live at `speed 1×` and the slider moves 0.3 → 4.3 over 4 s; all 22 cards show on a real fresh load with no StartRun; fourteen scripted cards open as "driving a live run" with samples growing (r-2 … r-15), the KV-spiral card's StartRun carries `preemption` and `session_turns_mean`, the spec-decode card's `spec_draft_tokens`; the nav link returns to the cards and back restores `#/showcase?script=rolling-hotspot`; Home's twelve report links answer 200; the harness stopped its 15 runs. **READY TO DEPLOY 945a727 sent to main** with the verdict lines. The two residuals became **U78** (sonnet, `claude/tl-probe-kind`, port 8187): the `least_kv_probe` card's StartRun is refused 400 because `least_kv_probe` is not a `RoutingKind` in the web config, so `policiesToWire` sends no routing; and Home's one console error is the browser's automatic `/favicon.ico` (a `data:` icon link in index.html). The dynamics count by the goal's definition: 12 selected · 12 showcased · **12 live** once U78 lands (11 tonight; `least_kv_probe` is the one still refused).
**22:05, the stop:** U78 landed (a73cb7c). **Final gate on master c22ea56, fresh build with reports: 50 passed, 0 failed, exit 0.** READY TO DEPLOY c22ea56 sent to main with the output; main deploys and reruns `QA_BASE=https://lbsim.ai node tools/qa/qa.js`. Seven units this session (U71–U76, U78), all landed through integrate.sh; the two facts found while briefing and the two the gate found are recorded above. Left for tomorrow, in order: U77 (main's cap decision), then the queue below unchanged. Two brief lessons for the productivity agent: line numbers in a brief drift under parallel landings (U78 got a stale range for `policiesToWire`; cite the symbol as well as the range), and `pkill -f` on a command line kills the agent's own shell (`pkill -x sim-run`).
**21:27, five spawned in one wave** (worktrees `lbsim-wt-*`, one local `sim-run serve` port each so the browser checks never collide): **U73** harness (`tools/qa/qa.js`, `serve-local.sh`, port 8181; default model); **U74** the live dashboard starts paced at the speed control's value instead of unpaced, so a 120 s scenario takes 120 s and the cursor follows the live edge; "speed 0×" is never a label (`speedLabel()`: paused / N× / unpaced) (port 8182; default); **U75** Showcase: the open walkthrough lives in the URL as `#/showcase?script=<id>` so the nav link, back button and a fresh load all return to the cards; the source decision uses the dashboard's own probe so the narration says live when the run is live; report on what opened `r-3` without a click (port 8183; default); **U72** engine keys the control panel has no field for (`preemption`, `session_*`, `spec_*`, `admission*`, `tenant*`, `failures`, `slo_classes`) ride on `ScenarioConfig.extra` through StartRun and back from `scenario.txt`, and `kv-spiral.json`'s scenario becomes `kv_spiral_never.txt` key for key, so demos 11 and 12 run live as themselves (port 8184; default); **U71** the three pre-U70 banner assertions, `.wt-mode`/`.tag-line` CSS, Home lists reports 1–12 with their finding titles (sonnet). Read while briefing, recorded here rather than fixed by hand: `Showcase.initialSource` answers `server` only from `serverMode().enabled` (env, storage or `?server=`), which the deployed build never sets, so every scripted walkthrough with a recording goes to the `replay` branch and hands `Dashboard` `data="auto"`; the dashboard's own probe then answers `server` because ListRuns responds, so on lbsim.ai the walkthroughs already drive live runs under a header that says "stepping through a replay" (U75 fixes the decision, U70's words were right). Main's "fresh load showed r-3 open with no click" came from `page.goto` to a hash-only URL on an already-open document, which is a `hashchange` and not a load; that is the same bug as the nav link (U75). **The Dockerfile builds reports 1–10 only; run-demos.sh has 11 (`11-preemption.html`) and 12 (`12-spec-decode.html`), so Home's links to those two will 404 on lbsim.ai until the cloud-owned Dockerfile adds them; told main.** Master had moved to da01aab (housekeeping) since the 21:12 stop; nothing else changed.
**20:53:** U58 landed (a94693b: `Dashboard` gains `run` and `data="server"`; `onRun` already existed; Showcase decides its source synchronously and no longer touches `window.history`; `.banner.rejected` added), so every script opens a live run when the server mode is on. U54 landed (5ba19ab: depth-limited parser, 30 s SSE write timeout, 503 past 8 live runs, checkpoint built under the lock and written after, `is_final` once, failed `advance_to` keeps its frames, Cloud Run paragraph in WIRE.md; deviation on item 3: a write error frees the thread and slot at once but the subscription lives until its lease expires, reaped on every request, because dropping it broke the Last-Event-ID reconnect test; two WIRE.md decisions to record: the 503 cap and "a write error does not end a subscription, only its lease does"). U40a landed (9ee9841). Review R2: U22, U52, U48 clean; two real U28 bugs (restart revives a disposed engine; a refused update leaves the refused value in the control panel) spawned as U61 (sonnet); two U22 design gaps (session turns bypass admission and the retry-budget denominator; decode-growth eviction can swap the queue head's own context out and straight back) spawned as U62. U25a stopped correctly on four one-line struct literals outside its files and resumed with them. S3 stays paused: every crate is under edit by an in-flight unit (sim-ingress by U59b next), so the simplification pass waits for U59b rather than run alongside it; cadence debt noted. Main deployed c3d1b84 as lbsim-00008-fbz at 20:49.
**20:50:** the count moved 0 → 10. U49 landed (9218eb9: `walkthroughRunner.ts` state machine over a `RunnerHandle`, 7/7 self-test; Showcase opens a script's `run` recording through the replay branch, `compare` noted not opened; U48's schema adopted mid-unit). Also landed: U57 (8475ed3, `updateBannerText`; `.banner.rejected` style owed, folded into U58), U56 (720db0e: `Sim::drain_frames`, `into_result` yields the undrained remainder, run.rs reattaches `st.frames` once at Finish for GetResult; Leaf trait untouched), U35 (8092332, trace replay, third rebase). U40a and U40b each refused twice at the golden file while five siblings landed; each authorised one more back-to-back rebase and integrate (U40a now, U40b after it). Stamps at 20:46/20:47 in the previous update were typed ahead of the clock (housekeeping caught it); restamped to the commit time 20:42. Spawned: U58 Showcase opens a live run (`run`/`onRun` props on Dashboard), U60 kv-spiral run ids (sonnet), U59a `Sim::apply_overrides` forward-only (the engine half; U59b the server arms after U54), U25a SLO classes in the engine, U26 speculative decoding knob, U31a failure injection in the engine, review R3 over the six merges since R2. U25a/U26/U31a/U59a all open `Replica::step` or the scenario keys; second lands rebases and re-refreshes html_md5.
**20:42:** U24 trace engine landed (3a350b1: rebased over U22 with four keep-both conflicts, `tracing.settle` placed before `follow_up` so a span sees the replica state at finish time; 21 html_md5 rows refreshed, zero fingerprint/events/summary changes; its agent could not force-push so the rebased history went up as `claude/tl-trace-engine-u22`, and the two stale branches were deleted from here, which does work now). U55 landed (e41ec81: live STEP_TIME a value in seconds on both paths, `kv_capacity_tokens.max(1.0)` in the exporter, `jsonl` served as text, WIRE.md decision 5 reworded; 40 sim-ingress tests). U35 was refused at the rebase stage because U24 landed between its rebase and its integrate; told to rebase once more and integrate immediately (its CSV had to be force-added past `.gitignore`'s `*.csv`, reported to main). **Main deployed c90bf35 as lbsim-00006-7sk and verified the live Ingress path on the public URL** (StartRun → RUNNING → SSE fleet rows → StopRun COMPLETE at 4x), so U28 is live for Issao at https://lbsim.ai/#/dashboard. Decision on main's note about Cloud Run's request-scoped CPU (a paced run with no subscriber stands still): documented as the idle rule it already is, in a WIRE.md paragraph added to U54; StartRun's default does not change. U22 and U24 both on master opens the engine fan-out: U25 and U26 spawn next.
**20:41:** U48 landed (a98d618): all ten selected dynamics have a walkthrough script with `run`/`compare` filled and checked against the export's ids; two latent bugs fixed on the way (walkthrough.ts read `import.meta.env` at module scope, so it could not be imported under node; the self-test's static `.ts` imports). U49 told to drop its stand-in on rebase. U52 landed (c90bf35, 20:36). Review R2 spawned over dcf77c8, 595ea7e, c90bf35, a98d618 (fourth code merge). U57 spawned 20:38 (`claude/tl-update-banner`, sonnet): `UpdateBanner` printed "nothing was re-simulated" for a refused update, found by U28 outside its files. Critical path is now U49 alone.
**Resume 20:26–20:29:** twelve units spawned in one wave. The five worktrees holding uncommitted work from agents killed by the rate limit (forecast-latency, forecast-load, mock-tags, walkthroughs, web-live) were resumed *in place*: each brief opens with `git status`, commits the recovered edits as a WIP checkpoint, rebases, and continues; U22 and U24 resumed from their pushed branches with their one-file fixes; U35 from its clean branch. Template v3 pasted verbatim except the Setup block, which for a resume names the existing worktree instead of `worktree add` (deviation recorded here, not in the template). Then the queue: U49 started now against two stand-ins (the `run`/`compare` schema fields as prose from schema.md, read through a local type until U48's walkthrough.ts lands; the live controller as it is on master), U54 and U55 from R1, and U56 from the main agent's decision on R1 item 8. Models: sonnet for U40a, U40b, U48, U52, U55, U56; default for the rest. `lbsim-integ-queue` was reset (three stale log files from the interrupted 17:35 run). The three units that add scenario keys (U22, U24, U35) each refresh the 20 `html_md5` rows because reports embed `Scenario::to_text`; every brief says to stop if anything but html_md5 moves.
**Dynamics, of 12 selected: 12 showcased · 12 live · 12 showcased-live, verified in headless Chromium on master c22ea56 (tools/qa, 50/50).** Earlier wording, kept for the history: 12 showcased on recordings · 10 live-runnable on the dashboard page · 10 showcased-live. Demos 11 (preemption) and 12 (speculation) fall short of live only because their scenario keys are not in the control panel's config mapping, so `scenarioFor(script)` drops them (U72, queued, small). Since U59b a walkthrough's `set` steps apply to the live run, forward-only; since U70 every page, panel tag, link and walkthrough header carries one of three words with its gloss: **mock** (browser-generated, invented numbers), **replay** (a recording of a real engine run), **live** (a simulation running on the server now). Live means the dynamic runs through the Ingress endpoint in the dashboard; showcased means a walkthrough script steps through it; showcased-live means the Showcase page itself drives a live run of the script's scenario. The goal, from Issao at 16:53: *"all selected dynamics are live demoable in the dashboard and in the showcase page."*
**20:57:** landed: U60 (2ce802c, demo 11's script opens its recording; not in the self-test's SELECTED_IDS, cosmetic), U59a (60d1421: `Scenario::with_override`/`override_kind`, `Sim::apply_overrides`, four tests; `export.rs::override_key` still carries its own copy of the round trip, a one-line delegation for a simplification pass), U40b (052d40e, renumbered to `13-forecast-latency` after U40a took 12; TTFT p99 3768 → 3291 ms, attainment 0.6235 → 0.6349 at load 0.8; catalog row in its section). Review R3: determinism holds on all six; U55, U56, U57 clean; three units from it: U63 (U35: unsorted trace rows underflow `t_ns -= origin`, a demo blocker; `offered_rps` is the synthetic rate in trace mode), U64 (U49 runner: `skip()` settles early on a live run, a cursor already past the step, a run ending before the step), U65 (U24 recorder: `steps` snapshots grow unbounded while any id is tracked); R3's unverified items on U24 (re-prefill after eviction recorded twice with no eviction span; sampler quotas consumed by warmup) are queued as U69, low. U59b spawned (server arms for UpdateWorkload/UpdatePolicies, forward-only, key policing by kind). Productivity agent's 20:5x measurements adopted: 13 merged in 25 min (32/hour); six stage-2 refusals on the golden file cost 24 agent-minutes, 10.7 of them waiting on my authorisation round trips, so (a) U66 (sonnet): `.gitattributes merge=union` on the golden file, the fingerprint run list moved into a data file with the same attribute, name-keyed rows, and html_md5 computed over the report with the scenario block stripped so a new key no longer moves 20 rows; (b) U68 (sonnet): integrate.sh skips the test and fingerprint stages when the diff touches only `*.md`; (c) template v4 from the next spawn, which makes a golden conflict the agent's to resolve without a round trip. Noted from the same section: U54 took a second task by message (15.8 min), against the sizing rule; not repeated.
**21:01:** U59b landed (7cc3395: UpdateWorkload/UpdatePolicies applied on the run thread between chunks, a paused run takes them within 50 ms, key policing by kind, rejected is HTTP 200 with the reason naming the key and the RPC to use, Rewind and GetTraces stay 501, WIRE.md updated; four HTTP tests). U62 landed (2896377: session turns go through `dispatch_pinned` → the same `shed()` admission step, counted in `first_attempts`; decode-growth eviction keeps the queue head; demo-11 numbers unchanged, so finding 8 needs only the note that admission now sees turns; `events` counts moved by one gateway event per turn). U61 landed (4f7ad62). U25a is one granted `class: 0,` line in tests/spec_decoding.rs from landing. U70 spawned (sonnet, web/): the three words mock / replay / live with one gloss each on every page, panel tag, link and walkthrough header. Nothing else spawns.
**21:12, the stop:** landed after the stop request, all inside the window: U25a SLO classes b852dba, U68 c97c509 (integrate.sh skips the gate for md-only diffs), U63 f73f95f (trace rows sorted; `Workload::offered_rps` in trace mode), U65 9938595 (recorder snapshots bounded), U26b 02e5dcb (demo 12 `12-spec-decode`, `spec-decode.json`, dynamic 15 per ARCHITECTURE's table), U64 2f89623 (runner: live skip, overshoot, early end; `RunnerHandle.ended?()`), U31a 33dfd18 (failure injection: `failures` key parsed by `Scenario::failure_events()`, `Replica.speed`/`down`, slow is silent, crash announced through the delayed view; a dispatch refused by a down replica is `TimeoutQueued`; the Timeout retry body moved into `Sim::abort`), U66 5150654 (`.gitattributes merge=union` on the two bench files, run list in `bench/fingerprint-runs.txt`, html_md5 over the report with the `<!-- scenario -->` block stripped: a new key moves no row from here on), U70 525d255 (the three words everywhere; `panelTagWord()`; no headless browser on PATH, routes confirmed structurally: `#/`, `#/dashboard`, `#/ab`, `#/showcase`). **Finding 9 draft (U31a), for docs/findings.md:** four replicas, `least_requests`, 30% of rated load, replica 3 at `slow=0.3` from t=60: p99 TTFT 2.41 s → 4.30 s, timeouts 24 → 38, and the slow replica still receives 15.8% of routed requests; at 0.3x a mean decode outlasts the 8 s client timeout, so most of what it takes never finishes, and timeouts cap its in-flight count, which is exactly why a load-reading router never backs off far enough. Housekeeping had stopped before this landed; the paragraph is here until it is recorded. Reviews R2 and R3 are complete; no monitor of the tech lead's is running. Stale remote branch `claude/tl-slo-classes` deleted.
**Restart 17:07 PDT:** a fresh tech lead resumed from this file. Twelve finished worktrees removed (7 GB back).
The U18 agent from the previous session turned out to be alive and committing (f8b1683 17:11, 5c5354b 17:14),
so it was not re-spawned; it is watched instead (see its section). Template v2 from the productivity agent
adopted from the first spawn; profile §5 item 1 was already done by U20 (gate 40 s), so its slot went to the
`tools/build.sh` fix in U45. Stale remote branches `claude/tl-*` from the previous session are content-merged
(`git cherry` says so) but could not be deleted from here; `tools/sync.sh` will keep listing their two old
markers until someone with push-delete rights removes them. Nothing in those branches is unmerged.
**17:45:** U18 landed (c0e9ea8, 38 unit + 8 HTTP tests, fingerprints PASS); U50 records its nine server decisions in
WIRE.md and U51 makes the trace encoder emit the TraceSpan fields the main agent added to metrics.proto at f5eddf1.
**17:35, pause:** U53 landed (6f9f913: exporter covers demos 7–10 as `7-no-decode`, `8-admission`, `9-fair-share`, `10-probes`, guarded by a test that parses run-demos.sh; 38 runs, 176 MB, 2.1 s). U22 and U24 are complete on their branches but refused by the gate for one file each (see their sections). R1 reviewed all four merges: eight findings on the U18 server (queued as U54), two on U23 (U55), U45 and U46 clean. The cloud agent deployed the replay dashboard as lbsim-00005-dzk and asks to be told when U28 + U48 + U49 make the first dynamic live and showcased; its note on `fleet.jsonl` served as octet-stream goes into U55.
**18:12:** U47 landed (128fc26, `tools/api-card.sh`, used for this round's briefs), U34 (e304990: `sim-run generate`, three p2c variants with the registry header, sha256, catalog append; first result: p2c d=3 scores 0 on h7 and 0.746 mean against p2c d=2's 0.762/0.872, so wider sampling is worse under stale telemetry, recorded in arena-implementation §6), U50 (96e88d0: the nine server decisions in WIRE.md), U51 (36f6538: encoder emits the new TraceSpan fields and `bucket`). Spawned: S3 simplify sim-ingress (excluding export.rs while U53 edits it), U40a `forecast_load` and U40b `forecast_latency` (sonnet, the two families Issao asked for at 16:22, byte-identical candidate sets to p2c so the comparison is honest), U35 trace-replay workload (M4, `workload = trace`, CSV). The engine units U25/U26/U31 stay queued until U22 and U24 land, because all three would be a fourth hand in `Replica::step`.
**17:58:** U23 landed (ede7b1f: replicas.jsonl, heatmap data real; it found the mock tag is unconditional in `Panel`, now U52), U45 (35c7918: build.sh round-robin slots, 420 s bound; 0.01 s / 3.01 s / exit 124 measured), U46 (072aa03: struct-update Scenario in eight tests, zero behaviour change). U53 spawned so the exporter covers demos 7–10, which U48's scripts need for run ids. Review agent R1 spawned over c0e9ea8, ede7b1f, 35c7918, 072aa03; its findings become units. Brief gaps fed back: web worktrees lack `node_modules` (symlink line now in web briefs); `cd` out of the worktree before integrate.sh (harmless getcwd noise otherwise).
**Critical path:** U18 done → U28 (in flight; can now be checked against `sim-run serve` on master) → U49
(walkthrough runner opens a live run) → the first "N live" number. U23 puts the heatmap on real data in
parallel; U22 opens the dynamics fan-out. Every unit is `model: default` unless its section says `sonnet`.

Legend: solid box = done; **bold** = in flight, with branch; plain = queued; dashed = waiting on Issao.
Edge labels name the stand-in that let the downstream unit start early, or why the edge could not be broken.

```mermaid
flowchart TD
  classDef done fill:#d9f2d9,stroke:#2e7d32,color:#000
  classDef flight fill:#fff3cd,stroke:#b26a00,color:#000,font-weight:bold
  classDef queued fill:#f4f4f2,stroke:#888,color:#000
  classDef blocked fill:#fde2e2,stroke:#c62828,color:#000,stroke-dasharray:4 3

  U01[U01 workspace split]:::done
  U02[U02 golden fingerprints]:::done
  U03[U03 wire contract WIRE.md]:::done
  U04[U04 policy trait + registry]:::done
  U05[U05 leases + idle guard]:::done
  U07[U07 bounded builds]:::done
  U09[U09 web transport client]:::done
  U10[U10 least_kv_probe]:::done
  U11[U11 deadline_aware]:::done
  U12[U12 fair_share]:::done
  U13[U13 wire export]:::done
  U14[U14 physics oracle M2a]:::done
  U15[U15 engine core: sim-model, frames, Sim, Leaf seam]:::done
  U16[U16 simplify 1+2]:::done

  U17[U17 replay source + Frame adapter]:::done
  U18[U18 live ingress server]:::done
  U19[U19 trace wire + export]:::done
  U20[U20 arena objective + catalog append]:::done
  U21[U21 disable_decode]:::done
  U22[U22 preemption + KV eviction]:::done

  U23[U23 per-replica rows + heatmap]:::done
  U24[U24 trace engine]:::done
  U35[U35 M4 trace replay workload]:::done
  U40a[U40a forecast_load]:::done
  U40b[U40b forecast_latency]:::done
  U04 --> U40a & U40b
  U40a & U40b --> U40
  U25[U25a SLO classes in the engine]:::done
  U25b[U25b per-class rows in report and export]:::queued
  U25 --> U25b
  U26[U26 speculative decoding knob]:::done
  U26b[U26b demo 12 spec decoding + walkthrough]:::done
  U26 --> U26b
  U70[U70 mock / replay / live labels on every page]:::done
  U71[U71 banner assertions, .wt-mode CSS, Home lists reports 1-12]:::done
  U72[U72 engine keys ride on ScenarioConfig.extra, so demos 11 and 12 run live]:::done
  U73[U73 tools/qa headless Chromium gate]:::done
  U76[U76 live controls before the run id; StopRun on page close; harness cleanup]:::done
  U73 --> U76
  U78[U78 least_kv_probe is a RoutingKind; favicon link]:::done
  U73 --> U78
  U77[U77 live-run cap counts busy or leased runs only; evict, then reap, idle-stopped runs]:::done
  U54 --> U77
  U79 -. "could not break: both edit drive() idle block and Server::reap" .-> U77
  U79[U79 Cloud Run: the live server degrades after a handful of runs; request log; lock order]:::done
  U54 --> U79
  U80[U80 Home in three sections: Live, Replay, Mock]:::done
  U81[U81 the top-right badge is accurate per route and state; harness asserts it]:::done
  U82[U82 build-process references out of user-facing text]:::done
  U83[U83 UI pass: every control does something in its mode; reviewer agent]:::done
  U84[U84+U85 Control panel header tag follows the mode; replay disables load, policies, cluster tabs as "recording"]:::done
  U83 --> U84
  U87[U87+U88 internal names out of the control panel and status bar; view-only sliders become readouts]:::done
  U89[U89 Showcase and A/B leftovers: ellipsis, no-A/B-yet copy, empty pager, refusal checkbox]:::done
  U83 --> U87 & U89
  U86[U86 first-sample race: serialised controls; poller past dispose; header word from first paint]:::done
  U76 --> U86
  U79 --> U86
  U71 --> U80 & U81 & U82
  U81 --> U83
  U80 & U82 --> U83
  U74 --> U76
  U74[U74 live dashboard starts paced; speed label never 0x]:::done
  U75[U75 Showcase: open script in the URL; source from the probe]:::done
  U28 --> U74
  U58 --> U75
  U73 --> U74 & U75 & U72
  U70 --> U71
  U26b --> U72
  U22 --> U72
  U52 --> U70
  U27[U27 prefix caching + sessions 9/10]:::queued
  U28[U28 web on live transport]:::done
  U29[U29 M3 leaf split + shard determinism]:::queued
  U30[U30 tiering 14 + disaggregation 15]:::queued
  U31[U31a failure injection in the engine]:::done
  U31b[U31b outlier ejection policy, demo 12, finding 9]:::queued
  U31 --> U31b
  U32[U32 autoscaling + multi-geo 12/13]:::queued
  U33[U33 M7 control analysis: Bode]:::queued
  U34[U34 arena generator loop]:::done
  U36[U36 leaf as a process]:::queued
  U37[U37 M9 scale validation]:::queued
  U38[U38 traffic shaping contrast report 5]:::queued
  U39[U39 model weights, MoE 17]:::queued
  U40[U40 forecasting policy families]:::queued
  U41[U41 simplify 3..n, one per four merges]:::queued
  U42[U42 SLO class targets: answered]:::done
  U44[U44 batch-class throughput over a longer window]:::queued
  U43[U43 rule-set version bump sign-off]:::blocked
  U45[U45 build.sh: round-robin slots + timeout]:::done
  U46[U46 tests build Scenario by struct update]:::done
  U47[U47 tools/api-card.sh]:::done
  U48[U48 showcase scripts for dynamics 1-10]:::done
  U49[U49 walkthrough runner]:::done
  U50[U50 WIRE.md: the server's nine decisions]:::done
  U51[U51 trace encoder: new TraceSpan fields]:::done
  U18 --> U50
  U52[U52 mode-aware mock tags]:::done
  U57[U57 UpdateBanner names a refused update]:::done
  U28 --> U57
  U58[U58 Showcase opens a live run]:::done
  U59a[U59a Sim::apply_overrides, forward-only]:::done
  U59b[U59b server UpdateWorkload/UpdatePolicies arms]:::done
  U60[U60 kv-spiral script run ids]:::done
  U49 --> U58
  U59a --> U59b
  U54 --> U59b
  U59b --> U58
  U22 --> U60
  U61[U61 useServerRun: restart after dispose, refused config]:::done
  U62[U62 session turns through admission; eviction keeps the queue head]:::done
  U28 --> U61
  U22 --> U62
  U63[U63 trace workload: sort rows, offered_rps in trace mode]:::done
  U64[U64 runner: skip on live, cursor past step, early end]:::done
  U65[U65 trace recorder: bounded snapshots]:::done
  U66[U66 golden file merges by union; html_md5 without the scenario block]:::done
  U68[U68 integrate.sh: md-only diffs skip the gate]:::done
  U69[U69 trace recorder: eviction span, warmup quotas]:::queued
  U35 --> U63
  U49 --> U64
  U24 --> U65 & U69
  U53[U53 exporter covers demos 7-10]:::done
  U54[U54 server hardening from review R1]:::done
  U55[U55 small fixes from R1 and cloud: STEP_TIME live value, kv cap, jsonl type]:::done
  U56[U56 Sim::drain_frames]:::done
  U18 --> U56
  U18 --> U54
  U23 --> U55
  U23 --> U52
  U21 --> U53 --> U48
  U52 --> U49
  U19 --> U51

  U01 --> U04 --> U10 & U11 & U12
  U01 --> U14 --> U22
  U01 --> U15 --> U22 & U18 & U23 & U24 & U29
  U03 --> U09 --> U17
  U03 --> U13 --> U17
  U03 -. "stand-in: WIRE.md prose, own types" .-> U17
  U05 --> U18
  U15 -. "stand-in: run to completion + fleet_rows until Sim" .-> U18
  U13 --> U18
  U13 --> U19
  U24 -. "stand-in: RequestTrace struct with fixtures" .-> U19
  U22 --> U25 & U26 & U27
  U22 -. "stand-in: Tracer in its own file, call sites only in step; second to land rebases" .-> U24
  U15 -. "could not break: Replica::step is one function" .-> U22
  U18 -. "stand-in: api.ts + an in-memory fake Ingress serving fleet.jsonl fixtures" .-> U28
  U17 --> U28
  U22 --> U30 --> U39
  U31 --> U32
  U32 --> U33
  U15 --> U33
  U04 --> U34
  U20 --> U34
  U16 --> U20
  U29 --> U36 --> U37
  U11 & U12 --> U38
  U42 --> U25
  U25 --> U44
  U20 --> U43
  U40 -. "stand-in: a template generator writing p2c variants, with the registry header" .-> U34
  U28 -. "stand-in: the controller as on master; 501 shown as a reason" .-> U49
  U48 -. "stand-in: run/compare fields as prose, local type" .-> U49
  U17 --> U48
```

## How to read a unit section

Each section names what the unit entails, the files it owns, its upstream and downstream units, the
stand-in interface that let it start before its upstream finished (if any), the agent and branch when in
flight, the stated ETA, and the definition of done: the test, fingerprint check, finding or deploy that
closes it. Every unit ends the way the first six dynamics did: `tools/build.sh test` green,
`./check-fingerprints.sh` PASS (or the baseline updated in the same commit with every moved number
explained), a scenario under `scenarios/` where a dynamic is involved, and a finding handed to
housekeeping for `docs/findings.md`.

## Done

### U01 workspace split
One crate became eleven under `crates/`, per ARCHITECTURE 10.8, with `tests/layering.rs` enforcing the
downward dependency direction and that `sim-ingress` cannot reach `sim-model` or `sim-physics`.
Byte-identical: 40 telemetry summaries and 16 reports. Commit 163995c.

### U02 golden fingerprints
`./check-fingerprints.sh` and `bench/golden-fingerprints.txt`: every demo, holdout and a probe run,
fingerprint, event count, summary md5 and report md5. The proof every later unit cites. 0efb8bd.

### U03 wire contract
`crates/sim-ingress/WIRE.md`: ingress.proto and subscription.proto as JSON over HTTP/1.1 with
server-sent events, proto3 canonical JSON encoding, SSE ids and Last-Event-ID reconnect. Decided over
gRPC to keep the zero-dependency one-second build. Two agents built against it at once. 0efb8bd, afc9ba1.

### U04 policy trait and registry
`sim-policy`: `RoutingPolicy` and `AdmissionPolicy` traits, one file per policy, one registry table
resolving `routing =` and `admission =` names, probes counted and charged. This is the fan-out point
and how the arena's generated policies register. Scenario gained admission, tenants and weights. 289cb22.

### U05 leases and idle guard
`sim-ingress` `lease.rs` and `idle.rs`: the wall-clock lease registry (clamped to Cloud Run's 900 s)
and the idle-shutdown decision (IDLE_SHUTDOWN_SECONDS, default 300). 18 tests. d688f8f.

### U06 homepage report links
Issao's instruction; the six reports linked from the dashboard homepage. Deployed as lbsim-00004. 16daf22.

### U07 bounded builds
Per-worktree target directories after the shared one handed a worktree another branch's rlibs;
`tools/build.sh` bounds concurrent builds to two machine-wide. 289cb22, 16daf22.

### U08 and U16 simplification passes
One per wave of merges, one crate each, zero behaviour change proven by the fingerprint script including
report hashes: `sim-report` 884→851 (4b29810), `sim-arena` 1358→1322 (fd81421).

### U09 web transport client
`web/src/lib/api.ts` and friends, against WIRE.md: bigint-exact uint64, SSE parser with reconnect,
33-case self-test, mock stays the default. 389f41e.

### U10 least_kv_probe · U11 deadline_aware · U12 fair_share
Three policies, one file each. least_kv_probe: live KV on d probes, CV 0.44 vs p2c 0.63 under 4 s
staleness (120e62d). deadline_aware: shed on expected queue wait, goodput 241→496 tok/s at 1.4x
(210b657). fair_share: windowed per-tenant tokens, the over-share tenant is 100% of what is shed, goodput
9,157→14,496 (072a932). Their scenarios enter the demo scripts with U21.

### U13 wire export
`sim-run export --demos`: the six demo groups as WIRE.md JSON under `runs/<id>/` (status, fleet.jsonl,
result.json as metrics.proto RunResult, index.json), 30 runs, 15.2 MB, 6.7 s; a test parses
subscription.proto so metric names and numbers cannot drift. 2354802.

### U14 physics oracle (M2, load-bearing half)
`sim-physics` `epoch.rs`: the analytic epoch advance with bandwidth and compute lines and speculation,
exact in u128 rationals, differential test against the naive loop on 10,000 random epochs, inversion,
calibration anchors (batch-1 10.248 ms vs 10.25; batch-256 7,758 tok/s vs 8,773, 11.6% low), 77x fewer
iterations measured. 35aa8a4.

### U15 engine core
`sim-model` holds `Replica` and the step; `RunResult.frames` gives per-sample window counts,
windowed sparse histograms and per-replica samples; `Sim::new/advance_to/into_result` makes the run
resumable; `sim-leaf-api` is the `Leaf` trait shaped by leaf.proto with `LocalLeaf` wrapping `Sim`
and a `sim-leaf` binary, per Issao's separate-process decision. Four commits, each fingerprint-PASS. ac8d1be.

## In flight

### U17 replay source and Frame adapter (done, 5077a34)
Load `runs/index.json` and `runs/<id>/{status.json,fleet.jsonl,result.json}` from the served directory
when no Ingress answers; map SubscriptionUpdate rows to the panels' `Frame`; play/pause/speed/step/scrub
local; rewind and update disabled with a visible reason. Files: `web/src/lib/replay.ts`, `adapter.ts`,
`useRun.ts` replay branch, `mode.ts`, minimal edits to Dashboard/Showcase/Compare/StatusBar to pick a
run and show the mode. Upstream U09, U13 (done). Downstream U28, U23. Agent on `claude/tl-replay`,
worktree `/home/agents/repo/lbsim-wt-replay`. Done: `npm run build` green; the six demo runs play in
the dashboard from static files with the load, throughput, latency, imbalance and KV panels real and
every other panel still marked mock; never commit exported runs (place an export under
`web/public/runs/` for local dev); deployed by the main agent.

**Landed.** 4f1013e merged as 5077a34: 15+33 self-test cases, headless Chromium shows the 30 demo runs
replaying with real fleet panels; open choices for U23/U28: the exporter buckets only measured records so
the first 15 s of a replay show no completions (export warm-up completions, or accept); `derive.ts` could
prefer the exact wire goodput/attainment fields; `web/public/runs/` should be gitignored (housekeeping).
Earlier resume note kept for the record: WIP pushed at 33af5f1 on origin/claude/tl-replay: `npm run build` green, tsc
clean, `replay.selftest.ts` 15/15, `api.selftest.ts` 33/33. Design: `adapter.ts` builds `hist.ts`
Histograms from the wire percentiles via a piecewise-linear CDF and keeps the exact wire p-values on the
frame, empty windows stay count 0 / NaN; a `ReplayEngine` implements the panels' FrameSource subset so
`RunHandle` stays drop-in; physics updates and restart refused with a reason, view-only SLO and
sample-rate changes still applied. Not yet: the README "Replay mode" section (written locally,
uncommitted), a browser render check against an export under `web/public/runs` (dev server serves it),
per-replica rows and the heatmap (that is U23). Next: finish the render check, commit the README and any
fix it surfaces, push, report branch, hash and open choices.

### U18 live ingress server (done, c0e9ea8)
`POST /v1/ingress/*` and the SSE `OpenSubscription` per WIRE.md, on the existing HTTP server, driving
`Sim` on a run thread paced by realtime_factor, leases from U05, idle guard wired to checkpoint-and-stop,
GetTraces returning U19's encoding when present. Files: `crates/sim-ingress/src/{server.rs,run.rs}`,
`lib.rs` route hook (keep `serve(dir, port)` and `is_health_path`), `tests/ingress_http.rs`. Upstream
U05, U13, U15 (done). Downstream U28. Agent on `claude/tl-ingress-server`, worktree
`/home/agents/repo/lbsim-wt-server`. ETA ~19:30 before the checkpoint. Done: the HTTP test starts a
run, subscribes, renews, closes, sees idle shutdown fire; `/health` untouched by run state.

**Landed 17:23** as c0e9ea8: three commits (49a8da6 lifecycle, 3f6fb71 subscriptions/SSE/leases/ring/Last-Event-ID, 6244632 idle checkpoint and health), 38 unit and 8 HTTP tests, fingerprints PASS; its report went to the main agent, who relayed nine decisions now being written into WIRE.md by U50. Correction to docs/iteration-profile.md: the `ingress_http` hang was this agent's first uncommitted draft (StepForward on a paused run waited forever), fixed before its first commit; the committed suite runs in about 10 s. Unimplemented RPCs answer 501: Rewind, UpdateWorkload, UpdatePolicies, GetTraces (one arm once U24 lands).
Earlier note kept for the record: **17:35, watched, not re-spawned.** The previous session's agent is alive in `/home/agents/repo/lbsim-wt-server`:
f8b1683 (17:11, lifecycle: StartRun/GetRun/ListRuns/StopRun/SetSpeed/StepForward/GetResult over TCP) and 5c5354b
(17:14, subscriptions over SSE, leases, replay ring) are on `origin/claude/tl-ingress-server`; the idle commit is
next. Its brief predates `tools/integrate.sh`, so when the branch stops moving with `tests/ingress_http.rs` green
the tech lead spawns a one-line integration unit for it; if no commit lands by 18:00 the unit is re-spawned from
the branch with this paragraph. Requirement added from the productivity agent's measurement: the HTTP test must
terminate on its own (a shutdown handle or a deadline on the accept loop), because a hung test held a build slot
for 600 s three times at 16:53.
Earlier resume note kept for the record: branch at origin/master 8cc21f2, nothing written; the design read-through is
done and these decisions are fixed: the run thread owns `Sim` (created in-thread, StartRun waits on a
channel for `Sim::new`'s verdict); `Mutex<RunState>` plus `Condvar` per run; subscriptions keyed `s-<n>`
with the client's `subscription_id` query parameter honoured on reconnect (api.ts already sends it); a
paced running run with no lease counts as idle, an unpaced running run counts busy; the frames' sparse
histograms give `from_merged_histogram: true`. Next: `run.rs` (registry, drive loop, frame → MetricRow),
then `server.rs` (JSON parser, `/v1/ingress` routing, SSE), then the `lib.rs` hook and
`tests/ingress_http.rs`; first commit "lifecycle" as soon as StartRun/GetRun/GetResult pass over TCP.
Brief essentials, three commits: (1) lifecycle: StartRun (scenario text + overrides via the export's
`override_key` round-trip), GetRun, ListRuns, StopRun, SetSpeed, StepForward capped at 60 simulated
seconds, GetResult via `wire::run_result`; (2) subscriptions: SSE `event: open` then `event: update` per
sample at the client's rate from `Sim::frames()`, `id:` sequence, Last-Event-ID replay from a 256-row
ring or 410, leases from `lease.rs`, renew/close; (3) idle: `IdleGuard` polled on the run thread, on
Shutdown write frames-so-far under `runs/<run_id>/`, STATE_PAUSED, `resume()` on a new subscription;
`/health` never touches run state (assert the frame count unchanged across 20 calls). Determinism test:
two starts of the same scenario give the same fingerprint.

### U19 trace wire and export (done, 3a00f92)
metrics.proto RequestTrace/TraceSpan as JSON per WIRE.md rules, `GetTraces` filters, traces in the export
under `runs/<id>/traces.jsonl` within the telemetry budget. Files: `crates/sim-metrics/src/trace.rs`
(the struct, a stand-in with fixtures until U24 fills it), `crates/sim-ingress/src/trace_wire.rs`,
`export.rs` additions, `tests/trace_wire.rs`. Upstream U13 (done); the edge from U24 is broken by the
struct stand-in. Downstream U18 (GetTraces route), U24, the dashboard trace panel. Agent on
`claude/tl-trace-wire`, worktree `/home/agents/repo/lbsim-wt-trace-wire`. Done: field-name test against
metrics.proto; export writes traces for a fixture run.

**Landed.** 9c1130a merged as 3a00f92: struct, sampler, encoder, GetTraces filters, `export_traces` with a
5 MiB default budget and manifest. Findings for the proto owner: TraceSpan lacks queued, batch_size,
step_ns, kv resident/capacity, bandwidth-vs-compute and the routing candidates; the TS types in
`web/src/lib/types.ts` differ from the proto in field names, units and prefixes (adapter work in U28).
Earlier resume note kept for the record: `crates/sim-metrics/src/trace.rs` is written and pushed at f94f284 (RequestTrace,
TraceSpan, SpanKind, ResourceState, TraceBucket, TraceSampler with windowed quotas,
`fixtures::sample_traces`, one unit test). Not yet: `crates/sim-ingress/src/trace_wire.rs` (encoder
emitting only proto TraceSpan and RequestTrace field names; `operation` from SpanKind, `concurrent_seqs`
= running, `kv_utilization` derived), `get_traces` filters and `parse_get_traces_request`,
`export.rs::export_traces` stratified like `sim-report`'s requests_csv within the telemetry budget plus
`export_run_with_traces`, `tests/trace_wire.rs` with the seven named tests
(trace_field_names_exist_in_metrics_proto, uint64_and_enum_encoding_follows_wire_rules,
get_traces_filters_by_outcome_min_e2e_tenant_and_limit, sampler_keeps_every_tail_bucket_at_low_rates,
sampler_is_deterministic_for_a_seed, export_traces_stays_within_budget_and_keeps_all_failures,
fixture_trace_round_trips_through_the_encoder), and the fingerprint run.

### U20 arena objective and catalog append (done, f6a87a9)
Per Issao, on the arena objective (TASKS.md entry 6, docs/arena.md 5b): *"You can remove this, I agreed
with this."* The score becomes the minimum over in-scope loads of goodput as a share of offered output
tokens, gated by the SLA cap as before; absolute goodput stays beside it as the diagnostic; a rule-set
version `RULE_SET` ("v2: cap 0.95 default, min over in-scope loads of gated goodput share") is recorded
in every `RunScore` and printed by `round_text`; `DEFAULT_SLA_CAP` becomes 0.95. Plus
`sim_arena::catalog::{CatalogRow, append, render_row, HEADER}` writing one row per authored policy to
`docs/policy-catalog.md` (header, verbatim: `| Name | Family | Idea | Status | Source | Score | Rule set
| Added |`; append into the matching `## <Family>` table, create the section if absent, idempotent,
escape pipes; the date comes from the caller), with a test that every table header in the committed
catalog equals `HEADER`. Also gates the two slow arena unit tests behind `#[ignore]` with a smoke round
in their place. Files: `crates/sim-arena/src/{lib.rs,catalog.rs}`, `tests/arena_rules.rs`,
`tests/arena_catalog.rs`. Upstream U16 (done). Downstream U34, U43. Agent on `claude/tl-arena-rules`,
worktree `/home/agents/repo/lbsim-wt-arena`. Done: `sim-run arena` ranks by share with the rule set
printed; the catalog tests pass against the committed file; `tools/build.sh test -p sim-arena` no
longer takes 80 s.

**Landed.** 21365f9 merged as f6a87a9: share objective v2, rule set recorded per score, cap 0.95, catalog
append with the header test, the two slow arena tests ignored with a smoke round (81.7 s → 0.5 s). Ranking
unchanged in order (p2c 0.762, round_robin 0.732, random 0.705, the two scanners 0); the worst load moved
from h6 to h4 for all three, the inversion arena.md 5b predicted. Docs owed to housekeeping: policy-catalog
Format wording and v1 scores, arena-implementation section 3, TASKS entry 6 closed.
Earlier resume note kept for the record: WIP commit 9d05e6d on origin/claude/tl-arena-rules builds: `catalog.rs`,
`RULE_SET` v2, `DEFAULT_SLA_CAP` 0.95, the share objective, `run_round_on`, `#[ignore]` on the two slow
arena tests plus `smoke_round_on_shortened_loads`, `tests/arena_rules.rs` and `tests/arena_catalog.rs`
written. Not yet run: the new tests, the full `tools/build.sh test`, `./check-fingerprints.sh`, the after
table of `sim-run arena --cap 0.95`. Baseline before: p2c 2652 > round_robin 2632 > random 2546 >
least_queue_tokens 0 > least_requests 0 (absolute goodput); `tools/build.sh test -p sim-arena` was
81.7 s warm. Next: run the tests, fix, fingerprints, capture the after ranking, turn the WIP into the
real commit, push.

### U21 disable_decode (done, c7f8c6a)
Per Issao at 16:22: *"we could get disable decode basically by setting HBM to infinity."* Scenario key
`disable_decode` zeroes the bandwidth term of the step cost in `CostModel::step_ns` and `rated_rps`; KV
accounting is unchanged so capacity still binds in tokens. Scenarios `route_round_robin_no_decode.txt`
and `route_p2c_no_decode.txt`, demo 7; demos 8–10 carry the U10–U12 scenarios; WIRE.md gained the export
section; 28 golden rows added, no existing number moved. Result: the ordering holds without decode, p2c
97.0% attainment against round robin 93.7%, ttft99 3.8 s against 7.2 s at identical throughput
18,986 tok/s. Finding 7 for docs/findings.md is owed to housekeeping (not sent at the checkpoint).

### U22 preemption and KV eviction (scope 7/8, the head of the dynamics fan-out)
**Landed 20:33** as dcf77c8 (resumed 20:29 from 574c02f): DEMOS row `11-preemption`, `demos_export_writes_every_group` (totals 38 → 40), html_md5 refreshed for the six new scenario lines with zero fingerprint/events/summary changes. Finding 8 sent to housekeeping. Brief gap recorded: the branch had not touched export.rs, so the agent rebased before editing and force-pushed with lease.
**Spawned 17:35** on `claude/tl-preemption`, worktree `/home/agents/repo/lbsim-wt-preemption`, model default,
ETA 18:00. Shares `Replica::step` and the sim-leaf loop with U24; whichever integrates second resolves the rebase.
**Paused 17:35, complete on the branch, gate-blocked.** Commit 574c02f on `origin/claude/tl-preemption`, worktree `/home/agents/repo/lbsim-wt-preemption` clean. Five tests pass, six scenario keys, `CostModel::swap_ns` with `KV_BYTES_PER_TOKEN`, `Replica::evict`, parked sessions, `preemptions` on `Frame`, demo 11, three golden rows added and zero changed. integrate.sh refused at stage 3 because U53's new `demos_table_mirrors_run_demos_sh` requires every run-demos.sh compare to have a `DEMOS` entry. **Resume:** in that worktree add `Demo { group: "11-preemption", files: &["kv_spiral_never.txt", "kv_spiral_swap.txt"], sweep: None }` to `crates/sim-ingress/src/export.rs` (~line 545), bump the counts in `demos_export_writes_the_ten_groups` if it hardcodes them, rebase onto origin/master (U24 also adds scenario keys; keep both), re-run integrate.sh. Design choices the agent made: running sequences are evicted only for decode growth (never for admission, to avoid ping-pong), a lone running sequence is never evicted, session turns are pinned to the holding replica, follow-up shapes come from a dedicated `session` stream, retry ids start at 1<<40. **Finding 8 draft for housekeeping:** at 2 sessions/s on four replicas (rated 45 rps), eight-turn sessions park context until a 30k-token cache is full of memory nobody is computing on; without eviction late-run attainment is 2% and p99 TTFT 36 s; swapping to DRAM at 50 GB/s serves the same load at 95% attainment, 84 ms p99 TTFT, 2.2 preemptions/s, 1,426 vs 780 tokens/s.
`PreemptionPolicy` per
scenario.proto: never, recompute, swap-to-DRAM, swap-else-recompute, with the victim choices; sessions so
KV fills at low qps; the death-spiral scenario and its survivor as demo 11. Upstream U14, U15, U21 (all
done). The edge from U15 could not be broken: `Replica::step` is one function and both units rewrite it.
Downstream U24–U27, U30. Branch `claude/tl-preemption`, worktree `/home/agents/repo/lbsim-wt-preemption`.

Brief essentials: scenario keys `preemption` (never | recompute | swap_to_dram | swap_else_recompute),
`preemption_victim` (newest | largest_kv | lowest_slo_class | latest_deadline), `session_turns_mean`,
`session_think_s` so parked sessions hold KV between turns at low qps; swap about 20 ms each way to
cluster DRAM and recompute re-charges the prefill of the evicted tokens, both through `CostModel`; use
U14's `EpochModel` where the step becomes an epoch. Scenarios `kv_spiral_never.txt` (collapses) and
`kv_spiral_swap.txt` (survives). Tests: `preemption_never_changes_a_run_with_no_kv_pressure` (the golden
file unchanged but for new rows), `eviction_frees_exactly_the_victim_kv`,
`recompute_charges_prefill_again`, `swap_charges_the_transfer`,
`the_spiral_collapses_without_preemption_and_recovers_with_it`. Files: `crates/sim-model/**`,
`crates/sim-physics/src/lib.rs` (CostModel only; `epoch.rs` untouched), `crates/sim-scenario/src/lib.rs`
keys plus `tests/scenario_parse.rs` KEYS, the two scenarios, `tests/preemption.rs`, `run-demos.sh`,
`check-fingerprints.sh`, `bench/golden-fingerprints.txt`. Done: the contrast pair in the report, finding 8
to housekeeping, golden updated with every moved number explained.

## Queued

### U23 per-replica export rows and the heatmap (done, ede7b1f)
**Landed 17:50** as ede7b1f: `replica_rows` from `frames[s].replicas`, `replicas.jsonl`, STEP_TIME and KV_TOKENS_RESIDENT constants, `replica_rows_follow_the_frames`, `loadRun` optional fetch, 17/17 web self-tests; frames align with the fleet series exactly. Finding: `MockTag` is rendered unconditionally by `Panel` (ui.tsx:39), so no panel can drop it; that is U52. Originally: **Spawned 17:35** on `claude/tl-replica-rows`, worktree `/home/agents/repo/lbsim-wt-replica-rows`, model default,
ETA 17:55. `export::replica_rows` from `RunResult.frames[s].replicas` (the seam at export.rs:251 is a stub):
one `SCOPE_REPLICA` update per replica per sample with QUEUED_SEQS 23, RUNNING_SEQS 22, KV_TOKENS_RESIDENT 21,
KV_UTILIZATION 20 as a fraction of `kv_capacity_tokens`, STEP_TIME 8; written to `runs/<group>/<run>/replicas.jsonl`
(a new line in WIRE.md's export section); `replay.ts` loads it and `adapter.ts` fills `Frame.replicas` so the
machine-level heatmap drops its mock tag on a replay. Tests: `replica_rows_follow_the_frames` (count = replicas ×
samples, sums equal the fleet row's QUEUED/RUNNING) and a replay self-test case. Upstream U15, U13, U17 (all done).

### U24 trace engine
**Landed 20:44** as 3a350b1 (see the 20:42 status line). Brief gaps it reported: integrate.sh reads `origin/<branch>`, so a rebase needs a force push, and its fingerprint diff prints computed on `<`, baseline on `>`.
**Re-spawned 20:29** in place from 7f79532 (html_md5 refresh, rebase over U22, integrate).
**Spawned 17:35** on `claude/tl-trace-engine`, worktree `/home/agents/repo/lbsim-wt-trace-engine`, model default,
ETA 18:05. **Paused 17:35, complete on the branch, gate-blocked.** Commit 7f79532 on `origin/claude/tl-trace-engine`, worktree `/home/agents/repo/lbsim-wt-trace-engine` clean. Four tests pass; sim-model, scenario_parse, trace_wire pass. integrate.sh refused at stage 4: the 20 `html_md5` rows moved because reports embed `Scenario::to_text`, which now emits `trace_sample_rate = 0`; every `fingerprint=`, `events=` and `summary_md5=` is unchanged. **Resume:** in that worktree refresh the 20 html_md5 values in `bench/golden-fingerprints.txt` (the fingerprint script's update path), commit with the explanation "report text gained one scenario line", rebase onto origin/master (U22 adds six keys to the same three places; whoever lands second re-refreshes html_md5), re-run integrate.sh. The inserted call lines in `Replica::step`: `r.tracer.admitted(id)` after the `running.push`, `r.tracer.prefill_chunk(id, take)` after `prefill_tokens += take`, `r.tracer.snapshot(ResourceSnapshot {..})` after `r.last_step_ns = step_ns`, `r.tracer.decode_step(id)` after `s.last_token_at = token_at`, `r.tracer.retired(id)` after `r.completed += 1`; plus `pub mod trace;`, a `tracer` field and `tracer_mut()`. Known limits: `candidates` holds probed replicas plus the chosen one (free stale views are not observable without a recorder in `RouteContext`); retries draw afresh.
Spans recorded in the step for a seeded, latency-stratified sample of requests (`trace_sample_rate`,
default 0 = off): ingress queue, routing decision with candidates and the stale view, replica queue, each prefill
chunk and decode step with batch size, running/queued, KV resident, step time, bandwidth- or compute-bound. Fills
U19's struct; `sim-run export` writes real traces. The edge from U22 is broken by a stand-in: the recorder lives in
its own file (`crates/sim-model/src/trace.rs`, engine-side events typed on sim-core only, converted to
`sim_metrics::trace` spans in sim-leaf) and touches `Replica::step` by inserted call lines only, so U22's rewrite
of the admission and retire loops rebases over it. Invariant: the fingerprint is byte-identical with tracing on
or off (the sampling draw comes from its own seeded stream). Downstream U19's export, the trace panel.

### U25 SLO classes
Per-class first-token and inter-token targets, class on every request, per-class goodput and attainment
in the scorecard and export. Upstream U22. Waits on U42 for the targets (default: interactive 2 s / 80 ms,
agent 5 s / 150 ms, batch 60 s / none).

### U26 speculative decoding knob
**Landed** as 0039c28; finding sent to housekeeping (2.61× at batch 4, 0.92× at batch 144, crossover B ≈ 72; rated_rps 234.8 → 240.2 on the p2c fleet). Reported, not fixed: the pre-decode eviction check assumes one new token per sequence per step, so at N>0 under KV pressure the cap can be overshot by a few tokens (queued as part of U69's trace/preemption follow-ups). `CostModel` also gained `spec_accept_rate`.
Scenario N/M, wired through `CostModel` using U14's compute line; the erosion at large batch as a
finding. Upstream U22 (shares CostModel).

### U27 prefix caching and sessions (9/10)
Session model per ARCHITECTURE §14 row 9 with fork-off and merge-back rates; prefix segment tree in the
replica; affinity routing; the failover-cascade scenario. Upstream U22.

### U28 web on the live transport
**Report, 20:37:** the seven self-test cases were already written; the build was red on a runtime-module type import, fixed; one real gap fixed test-first (a rejected update was mirrored onto `refused` so `ServerBanner` shows "not applied: reason"); `useServerRun.ts` is 633 lines, not 443 (brief error). Left for U57: `UpdateBanner` in PlaybackBar.tsx.
**Landed 20:34** as 595ea7e (resumed 20:29 in place from eight uncommitted files: the live path is checked end to end through the fake Ingress, and a refused update is visible). Report pending; details follow in the next graph update.
**Spawned 17:35** on `claude/tl-web-live`, worktree `/home/agents/repo/lbsim-wt-web-live`, model default, ETA 18:00.
**Paused 17:35.** Branch `claude/tl-web-live`, worktree `/home/agents/repo/lbsim-wt-web-live`; a "WIP: U28 pause checkpoint" commit was requested; the agent had been told U18 is on master, the nine server decisions, and to rebase over U23's adapter change. **Resume:** from the WIP commit, finish the six self-test cases, `npm run build`, integrate.
`useServerRun.ts` (exists, shaped to `RunHandle`) becomes the dashboard's source when `ListRuns` answers:
`dataModeFrom` picks `server`, the load-test page starts a run from the control panel's config, the fleet
subscription feeds the same `adapter.ts` path replay uses, mock markers drop only on wired panels,
UpdateWorkload/UpdatePolicies enabled, rewind refused with a reason until the server supports it. The edge from U18
is broken by a stand-in: `web/src/lib/fakeIngress.ts`, an in-memory `FetchLike` that answers the WIRE.md RPCs and
streams `fleet.jsonl` fixtures as SSE, drives `useServerRun`'s non-hook controller in a self-test. U49 closes the loop
against the real server once both are on master. Upstream U17 (done), U18 (stand-in).

### U29 M3 leaf split — FUTURE WORK, out of every lane and queue (Issao, 2026-09-07 21:32 PDT)
*"let's not do sharding then, leave it recorded for future work."* Reason: the single-threaded core does 10k replicas at 1.5–1.9× realtime on one core (M9 measurement below) and the 5×10k-GPU target is 6,250 replicas, so sharding is the lever only past that; the Leaf seam (U15) stays as the boundary it would plug into.
Routing out of the loop through the `Leaf` trait; shard barrier; the determinism-across-shards test at
1, 4, 16 shards. Upstream U15. Downstream U36.

### U30 tiering and disaggregation (14/15)
Cluster-pooled DRAM/SSD owned by Ingress with the bloom-filter residency hint Issao described; prefill and
decode pools with KV transfer over the modelled fabric. Upstream U22.

### U31 failure injection and gray failure (11)
FailureSpec events, health vector, outlier ejection policy. Downstream U32, U38.

### U32 autoscaling and multi-geo (12/13)
Replica lifecycle with turn-up delay, warm pools, diurnal load per cluster, cascading failure. Upstream U31.

### U33 M7 control analysis
Perturbation input, frequency sweep, empirical Bode plot predicting oscillation onset. Upstream U15, U32.

### U34 arena generator loop (done, e304990)
**Landed 18:05.** `sim-run generate --family routing --variant <p2c_d3|p2c_d4|least_kv_p2c> [--keep --date]`: header line exact, in-crate sha256, `--keep` rebuilds and scores through a child invocation, catalog row appended; no generated file committed. First measured result: p2c_d3 0.000 min (h7) / 0.746 mean vs p2c 0.762 / 0.872. Originally: **Spawned 17:35** on `claude/tl-generator`, worktree `/home/agents/repo/lbsim-wt-generator`, model default, ETA 18:05.
`sim-run generate`: writes a policy file into `crates/sim-policy/src/gen_<name>.rs` whose first line is the registry
header `//! lbsim-policy: routing names=<name>` (required by the main agent: `build.rs` reads exactly that line),
rebuilds through `tools/build.sh`, runs the round, records the source hash, appends the catalog row through
`sim_arena::catalog::append`. The edge from U40 is broken by a stand-in: a template generator producing p2c variants
(d = 3, 4, and least-KV keyed) so the loop is exercised end to end before forecasting families exist. Generated files
are committed only when the generator is asked to keep them. Upstream U04, U20 (done), U40 (stand-in).

### U35 M4 trace replay workload
**Landed** as 8092332 after a third rebase. The CSV was force-added past `.gitignore`'s `*.csv`; narrowing that rule is reported to main.
**20:42, at its final rebase.** Branch `claude/tl-trace-workload` at d289d09: keys `workload`/`trace_file`, trace mode behind `next_gap_ns`/`make` with a lazy CSV load and zero random draws, arrivals stop after the last row; `scenarios/traces/sample.csv` (195 rows, burst at 12–14 s), `scenarios/trace_replay.txt`; both tests pass, only html_md5 moved. Choices: trace times are offsets from the first row (the leaf fires the first arrival at start); tenant clamped; `is_long` = prompt above the geometric mean of the two prompt means; a missing trace panics with the path. Refused once at the rebase stage (U24 landed in between); one more rebase and integrate authorised.
**Re-spawned 20:29** from the clean branch (nothing had been written before the pause); html_md5 refresh expected for the two new keys.
**Spawned 18:12** on `claude/tl-trace-workload`, ETA 18:35. **Paused 17:35.** Branch `claude/tl-trace-workload`, worktree `/home/agents/repo/lbsim-wt-trace-workload`; WIP checkpoint requested (spawned 18:12, so likely early). **Resume:** from the WIP commit, per the brief; add the two keys at the end of the three places and rebase over U22/U24. `workload = trace`, `trace_file` CSV (`t_s,prompt_tokens,output_tokens,tenant`), arrivals stop at the end of the trace, no random draw in trace mode so synthetic fingerprints cannot move; `scenarios/traces/sample.csv` and `scenarios/trace_replay.txt`. Fits behind `Workload::next_gap_ns`/`make` without touching sim-leaf.

### U40a `forecast_load` · U40b `forecast_latency` (`model: sonnet`)
**Both landed:** U40a 9ee9841, U40b 052d40e (catalog row: `| forecast_latency | routing | predicted TTFT = (queued + own prefill tokens) × last step time/token | landed | crates/sim-policy/src/forecast_latency.rs | — | v2 | 2026-09-06 |`; its fingerprint row is a plain `run` because `compare` refuses differing workloads).
**U40a landed** as 9ee9841 (catalog row: `| forecast_load | routing | queue_depth_extrapolation | landed | crates/sim-policy/src/forecast_load.rs | — | v2 | 2026-09-06 |`). U40b authorised for its final rebase after it.
**Both complete on their branches, refused twice each at the golden-file rebase** (U40a b47ca16: staleness CV 0.1449 → 0.1414; U40b f341424: at load 0.8 TTFT p99 3768 → 3291 ms, attainment 0.6235 → 0.6349, labelled `12-forecast-*` as plain `run` rows because `compare` refuses differing workloads). One more back-to-back rebase and integrate each, U40a first. Catalog rows in their reports go to housekeeping at landing.
**Re-spawned 20:29** in place from three untracked files each (policy, scenario, test); each adds a check-fingerprints.sh line and golden rows; second to land rebases.
**Spawned 18:12** on `claude/tl-forecast-load` and `claude/tl-forecast-latency`, ETA 18:30. **Paused 17:35.** Branches `claude/tl-forecast-load` (worktree `/home/agents/repo/lbsim-wt-forecast-load`) and `claude/tl-forecast-latency` (`/home/agents/repo/lbsim-wt-forecast-latency`); WIP checkpoints requested (spawned 18:12). **Resume:** from the WIP commits, per the briefs; both append a `run` line and golden rows, second to land rebases. Per Issao at 16:22, the two forecasting families as policy files against the trait: `forecast_load` extrapolates queued tokens from the last two views' slope over the view's age; `forecast_latency` routes on predicted TTFT from queued tokens, the request's own prefill and the candidate's last step. Both keep p2c's candidate draw so candidate sets are byte-identical at a seed. Each: scenario, tests against p2c under staleness / long prompts, golden rows appended, catalog row to housekeeping.

### U36 leaf as a process · U37 M9 scale validation · U38 shaping contrast
report · U39 model weights and MoE (17) · U40 forecasting policy families · U41 simplification cadence
Each as named in docs/execution-plan.md and docs/scope-today.md; none started; each becomes a section
when it is specified to the five-field standard.

**M9 first measurement (2026-09-07 21:20 PDT, main, release build, one core, route_p2c at ~0.3 offered/capacity):** 1,000 replicas @ 2,500 rps: 120 sim-s in 2.1 s wall = 57× realtime, 95 MB peak, 9.0 M events. 10,000 replicas @ 25,000 rps: 60 sim-s in 38.8 s = 1.5×, 30 sim-s in 16.6 s = 1.8×, 580 MB peak, 1.14 M events/s; the 120 s run aborted at the 50 M event tripwire with 44 % remaining (U97 scales it). 10k replicas on one core is 1.5–1.9× realtime, just under the 2× target; Leaf sharding (M3/U29) is the lever.

### U45 tools/build.sh: round-robin slots and a bounded hold (fix-once, `model: sonnet`, done 35c7918)
**Landed 17:52** as 35c7918 with `tools/build-slots.test.sh`: slot 2 taken in 0.01 s while slot 1 is held, a two-slot wait resolves in 3.01 s, a bounded run exits 124 at 2 s. Originally: **Spawned 17:35** on `claude/tl-build-slots`, worktree `/home/agents/repo/lbsim-wt-build-slots`, ETA 17:50. From the
productivity agent's measurement at 17:20: three agents lost 600 s each because a hung test held slot 1 and the
fallback pinned every waiter to slot 1 while slot 2 sat free. Fix: poll the slots round-robin once a second, and exec
cargo under `timeout -k 5 ${LBSIM_BUILD_TIMEOUT:-420}` so a hang returns 124 inside the agent's turn and releases the
slot. Done: a shell check that a waiter takes slot 2 when slot 1 is held, and that a held run exits 124 at the bound.

### U46 tests build `Scenario` by struct update (fix-once, `model: sonnet`, done 072aa03)
**Landed 17:55** as 072aa03; eight files, pass counts unchanged. Originally: **Spawned 17:35** on `claude/tl-scenario-default`, worktree `/home/agents/repo/lbsim-wt-scenario-default`, ETA 17:50.
Profile §4 row 1: eight test files build `Scenario` as an exhaustive literal, so every branch adding a key breaks every
other branch at rebase. Each becomes `tests/common::small()` plus field sets, or `..Scenario::default()`. The
exhaustive literal in `tests/scenario_parse.rs` stays: it is the round-trip guard and must name every key. Zero
behaviour change: every test passes unchanged.

### U47 `tools/api-card.sh <crate>` (fix-once, `model: sonnet`, done 128fc26)
**Landed 18:00.** `tools/api-card.sh <crate|tests|web> [pattern] [context]`; five checks in tools/api-card.test.sh. Originally: **Spawned 17:35** on `claude/tl-api-card`, worktree `/home/agents/repo/lbsim-wt-api-card`, ETA 17:45. Profile §5 row 2:
prints every `pub` item of a crate with `file:line` and its signature line, so a brief can carry excerpts cheaply.
Done: `tools/api-card.sh sim-model` lists `Replica::step` at its line; `tools/api-card.sh sim-model step` prints the
matching items with 12 lines of context.

### U48 showcase scripts for the ten selected dynamics (`model: sonnet`)
**Landed 20:41** as a98d618: the four demo 7–10 scripts already carried the right ids; the agent removed the self-test's pending-id skip so all ten scripts get the strict allow-list check (68/68), and fixed two latent bugs (module-scope `import.meta.env`, static `.ts` imports) that had stopped the self-test and the build from running.
**Re-spawned 20:29** in place from 14 uncommitted files; the demo 7–10 ids and the slug rule are in the brief.
**Spawned 17:35** on `claude/tl-walkthroughs`, worktree `/home/agents/repo/lbsim-wt-walkthroughs`, ETA 18:00. **Paused 17:35.** Branch `claude/tl-walkthroughs`, worktree `/home/agents/repo/lbsim-wt-walkthroughs`; WIP checkpoint requested. The demo 7–10 run ids now exist (U53): `7-no-decode/round-robin-no-decode`, `7-no-decode/p2c-no-decode`, `8-admission/accept-all`, `8-admission/deadline-aware`, `9-fair-share/tenants-accept-all`, `9-fair-share/fair-share`, `10-probes/p2c`, `10-probes/least-kv-probe`. **Resume:** from the WIP commit, fill those ids into the four scripts, run the self-test and `npm run build`, integrate. One
`web/public/walkthroughs/<id>.json` per selected dynamic (findings 1–6, demos 7–10), each step quoting the finding's
numbers, with two new optional schema fields `run` and `compare` naming the exported demo run ids the step plays;
`index.json` cards updated so every selected dynamic has a script; a self-test validates monotone `at_sim_s` and that
every `run` id is one `sim-run export --demos` writes. Content only; the runner that opens `run` is U49.

### U50 WIRE.md records the live server's decisions (`model: sonnet`, done 96e88d0)
**Landed 18:05**, nine decisions in their sections; the H2 "What the first server supports" kept because run.rs and server.rs cite it. Originally: **Spawned 17:45** on `claude/tl-wire-decisions`, ETA 17:55. The nine decisions U18 made (reconnect carries
`subscription_id` with Last-Event-ID, 410 when dead; idle = paused, complete, or paced without a lease; the idle
checkpoint is the export documents, not a snapshot; `from_merged_histogram` true on live rows; replica STEP_TIME a
one-sample distribution; StopRun finalises COMPLETE; lease default 60 s, 0 dead; 501 for unimplemented RPCs;
1 MiB body cap) written into the sections of `crates/sim-ingress/WIRE.md` they belong to.

### U51 trace encoder emits the new TraceSpan fields (`model: sonnet`, done 36f6538)
**Landed 18:07**: batch_size, queued, kv_tokens_resident, kv_capacity, step_ns, bound (STEP_BOUND_*), candidates and stale_view_age_ns on routing spans, bucket (TRACE_BUCKET_*); 8/8 trace_wire tests. Originally: **Spawned 17:45** on `claude/tl-trace-fields`, ETA 17:55. metrics.proto at f5eddf1 gave TraceSpan the resource state
(batch_size 14 … stale_view_age_ns 21, `StepBound bound`) and RequestTrace a `bucket`; `trace_wire.rs` emits them
and `tests/trace_wire.rs` checks the names against the proto. `web/src/lib/types.ts` follows in U49 or the trace
panel unit. U19's section stands; U24 fills the values.

### U52 mode-aware mock tags (`model: sonnet`)
**Landed 20:36** as c90bf35: `wired.ts`, `realness()` in `Panel`, four panels declaring their reads; `replicas[].state` kept mock; 7/7 self-test.
**Re-spawned 20:29** in place from seven uncommitted files.
**Spawned 17:58** on `claude/tl-mock-tags`, ETA 18:15. **Paused 17:35.** Branch `claude/tl-mock-tags`, worktree `/home/agents/repo/lbsim-wt-mock-tags`; WIP checkpoint requested. **Resume:** from the WIP commit, per the brief. `web/src/lib/wired.ts` names the Frame and ReplicaSample fields
the engine supplies; each observe panel declares what it reads and passes `realness()` to `Panel`, which hides the tag
when everything read is wired and titles it with the still-mock fields when partial. Files disjoint from U28's.

### U53 exporter covers demos 7–10 (`model: sonnet`)
**Spawned 17:58** on `claude/tl-export-demos`, ETA 18:10. Four `Demo` entries mirroring run-demos.sh, guarded by a
test that parses run-demos.sh so the two cannot drift; the eight new run ids go to U48's scripts.

### S3 simplification pass, sim-ingress
**Paused 17:35, cancelled for now.** Branch `claude/simplify-3`, worktree `/home/agents/repo/lbsim-wt-simplify-3`; told to stop, checkpoint only if edits existed. **Resume:** re-spawn after U54 lands (it edits the same files), never alongside another simplification. **Spawned 18:12** on `claude/simplify-3`, the crate that grew 1,800 lines today; export.rs excluded while U53 edits it; tests unchanged are the proof.

### U54 server hardening from review R1 (queued, default model)
**Landed** as 5ba19ab (46 unit + 12 HTTP tests). See the status line for the item-3 deviation and the two WIRE.md decisions owed to the record.
**20:42, scope +1:** also owns one WIRE.md paragraph under "Leases and idle shutdown" documenting Cloud Run's request-scoped CPU: a paced run advances only while a request (an SSE subscription counts) is in flight, which is the idle rule; fire-and-forget callers use an unpaced run. No code change.
**Spawned 20:29** on `claude/tl-hardening`, worktree `/home/agents/repo/lbsim-wt-hardening`, model default, ETA 20:50. Tests first for items 1, 2, 3, 6 in tests/ingress_http.rs; 4, 5, 7 as unit tests in run.rs. Shares run.rs/lib.rs lines with U55 and U56 on disjoint ranges; second to land rebases.
From R1's read-only review of c0e9ea8, ranked: (1) `server.rs:592-657` the JSON `Parser::value` recurses without a depth limit; ~10 KB of `[` overflows the thread stack and aborts the process, losing every run: add a depth argument, error past 64. (2) SSE sockets have no write timeout (`lib.rs:125`, `server.rs:366,375`): a client that stops reading pins a thread, a MAX_CONNECTIONS slot and a lease forever; `set_write_timeout(30 s)` before `sse_head`. (3) On write error `?` returns without removing the `Sub`; ring and lease leak: expire leases in `open_subscription` and at line 308. (4) No cap on runs (`run.rs:243-287`); unpaced runs are never reaped: refuse with 503 past a small constant of non-terminal runs. (5) `checkpoint` does fs writes under the run mutex while `server.rs:326` holds `subs` then the run lock: build strings under the lock, write after `drop(st)`. (6) `is_final` can be sent twice for the last frame at natural end: `is_final = (terminal || stop_requested) && j == n`. (7) On `advance_to` error frames closed in that chunk are dropped: copy before `Failed`. Tests first for 1, 2, 3, 6 in `tests/ingress_http.rs`. R1's item 8 (frames cloned out of `engine.frames()`, 2x memory, unbounded; needs a sim-leaf drain API) is a crate-boundary decision reported to the main agent, not made here.

### U55 small fixes from R1 and the cloud agent (queued, `model: sonnet`)
**Landed 20:45** as e41ec81: tests `live_replica_step_time_is_a_value_in_seconds` and `jsonl_is_served_as_text`; adapter.ts untouched (it already reads a value).
**Spawned 20:29** on `claude/tl-small-fixes`, worktree `/home/agents/repo/lbsim-wt-small-fixes`, `model: sonnet`, ETA 20:40.
(a) `run.rs:566-579` emits replica `METRIC_STEP_TIME` as a nanosecond distribution while the exporter (U23) emits a seconds value and `adapter.ts` reads values, so the live heatmap's step time is NaN: emit `value(METRIC_STEP_TIME, last_step_ns / 1e9)` live and update WIRE.md decision 5 accordingly. (b) `export.rs:261` lacks `.max(1.0)` on `kv_capacity_tokens` (run.rs:551 has it). (c) `lib.rs` content-type table has no `jsonl`, so `fleet.jsonl` is served as octet-stream: add `text/plain; charset=utf-8`.

### U49 walkthrough runner on replay and live runs
**Landed** as 9218eb9 (see the status line). Brief gaps it reported: the handle needs `play()`; Dashboard's replay picker is URL-driven only, so Showcase pinned `?run=` through `history.replaceState` (removed by U58).
**Spawned 20:29** on `claude/tl-runner`, worktree `/home/agents/repo/lbsim-wt-runner`, model default, ETA 20:55. Files: `web/src/pages/Showcase.tsx`, new `web/src/lib/walkthroughRunner.ts` and its self-test. Both edges broken by stand-ins (see the graph): it reads `run`/`compare` through a local type until U48 lands, and codes against the live controller as it is on master. Its self-test drives a fake handle: set-before-advance, replay refusal with a reason, live 501 as a reason, done after the last step.
Queued behind U28 and U48. `Showcase.tsx` opens a script's `run` through the replay source or, when the Ingress
answers, starts it live; `set` steps call UpdateWorkload/UpdatePolicies in live mode and are refused with the reason
in replay mode. This is the unit that turns "N live and showcased" from 0 to 10.

### U56 `Sim::drain_frames` (from R1 item 8, decided by the main agent 20:28; `model: sonnet`)
**Spawned 20:29** on `claude/tl-frame-drain`, worktree `/home/agents/repo/lbsim-wt-frame-drain`, ETA 20:45. The run thread clones every new frame out of `engine.frames()` and the engine keeps its own copy, so a live run holds every frame twice, unbounded. `pub fn drain_frames(&mut self) -> Vec<Frame>` on `Sim`, used at run.rs 409/427; `RunResult.frames` stays complete for a drained run (the agent picks the smaller of two ways and reports which); `frames()` stays for the export path so the golden file proves nothing moved. The `Leaf` trait is not touched; if it had to be, that goes back to the main agent. Test: `draining_yields_the_same_frames_as_not_draining`.
### U57 `UpdateBanner` names a refused update (`model: sonnet`)
**Landed** as 8475ed3; `.banner.rejected` CSS rule owed (styles.css was not owned), folded into U58.
**Spawned 20:38** on `claude/tl-update-banner`, worktree `/home/agents/repo/lbsim-wt-update-banner`, ETA 20:50. Found by U28: `UpdateBanner` reads only `requiredResimulation`, so a 501 or a replay refusal renders as "nothing was re-simulated". A pure `updateBannerText(u)` in `web/src/lib/updateBanner.ts` with a self-test (rejected / resim / applied), rendered with a `rejected` class.

### R2 review of the resume's first four code merges (read-only)
**Spawned 20:41** over dcf77c8 (U22), 595ea7e (U28), c90bf35 (U52), a98d618 (U48): determinism first, then concrete-input bugs, live-path leaks, unsupported claims. Findings become units.
### U58 Showcase opens a live run
**Landed** as a94693b. Not done: no reachability probe before choosing `server` (needs an `IngressClient`); `compare` stays a note.
**Spawned** on `claude/tl-showcase-live`, model default. `Dashboard` gains `run?: string` and `onRun?: (run: RunHandle) => void`; Showcase hands the live or replay handle to its runner instead of pinning `?run=`; live when `dataModeFrom` says server, from `scenarioFor(script)`; adds `.banner.rejected`. Files: Dashboard.tsx, Showcase.tsx, walkthroughRunner.ts (interface only), styles.css.

### U59a `Sim::apply_overrides`, forward-only (engine half of live updates)
**Landed** as 60d1421. Exact signatures: `sim_scenario::OverrideKind { Workload, Policy, Structural }`; `Scenario::with_override(&self, key, value) -> Result<Scenario, String>`; `Scenario::override_kind(key) -> OverrideKind` (unknown = Structural); `sim_leaf::Applied { changed: Vec<String>, at: Nanos }`; `Sim::apply_overrides(&mut self, &[(&str, &str)]) -> Result<Applied, String>`. **U59b spawned** on `claude/tl-update-arms`, model default: pending update applied between chunks on the run thread, `Run::update` blocks like `step_forward`, key policing by kind, rejected is HTTP 200 with the reason, Rewind and GetTraces stay 501, WIRE.md decision; four HTTP tests.
**Spawned** on `claude/tl-live-overrides`, model default. `Scenario::with_override` and `override_kind` (workload / policy / structural) in sim-scenario; `Sim::apply_overrides(&[(key, value)]) -> Result<Applied>` refuses structural keys whole, replaces `sc`, rebuilds router/admission through `make_routing`/`make_admission` when a policy key changed; streams untouched. Four tests in tests/live_overrides.rs. **U59b** (queued, after U54 and U59a): server.rs 184 arms for UpdateWorkload/UpdatePolicies calling it on the run thread, `UpdateResponse { accepted, required_resimulation: false, rewound_to_s: now, rejected_reason }`, Rewind stays 501; WIRE.md decision.

### U60 kv-spiral script run ids (`model: sonnet`)
**Landed** as 2ce802c (71/71 self-test).
**Spawned** on `claude/tl-kv-spiral-script`: `run` = `11-preemption/kv-spiral-never`, `compare` = `11-preemption/kv-spiral-swap`, finding-8 numbers in the steps, the ids added to the self-test allow-list. Makes the count 11 of 11 by the current definition.

### U25a SLO classes in the engine
**Stopped and resumed:** `class: u8` on `Request`/`RequestRecord` needs one `class: 0,` line in tests/preemption.rs, tests/trace_wire.rs and two sites in crates/sim-metrics/src/trace.rs; ownership granted, the design kept.
**Spawned** on `claude/tl-slo-classes`, model default. Key `slo_classes = interactive:0.7,agent:0.2,batch:0.1`; targets are U42's constants; class drawn from its own stream only when on; `class` on Request and RequestRecord; per-class `class_goodput_tokens_s`/`class_attainment` on RunResult; four tests including byte-identity when off. **U25b** (queued): per-class rows in the report table and the export.

### U26 speculative decoding knob
**Spawned** on `claude/tl-spec-decode`, model default. Keys `spec_draft_tokens`, `spec_accept_rate`; a deterministic per-sequence fractional accumulator advances (1 − α^(N+1))/(1 − α) tokens per step (a geometric draw from a named stream is the later refinement); `CostModel.spec_draft_tokens` adds `decoding × N / prefill_tokens_per_s` to the compute term; tests on both sides of the crossover, the finding paragraph in the report; demo wiring is a follow-up.

### U31a failure injection in the engine
**Spawned** on `claude/tl-failures`, model default. Key `failures = t=60,replica=2,kind=slow=0.3;...` parsed into `Vec<FailureEvent>`; `Replica.speed` and `down`; slow is silent (gray), hang is speed 0, crash fails in-flight requests and is announced through the *delayed* view (`ejected: true`), `until` restores. Four tests; finding-9 draft from the slow-replica numbers. **U31b** (queued): outlier ejection policy, demo 12 pair, walkthrough.

### R3 review of the six merges since R2 (read-only)
**Spawned** over 3a350b1, e41ec81, 9218eb9, 8475ed3, 720db0e, 8092332; determinism first (tracing on/off, trace mode draws), drain-vs-ring frame loss, runner overshoot.
### U61 useServerRun fixes from R2 (`model: sonnet`)
**Landed** as 4f7ad62; 9/9 self-test; case (e)'s stale assertion (which encoded the bug) corrected.
**Spawned** on `claude/tl-server-run-fixes`: `restart()` checks `disposed` and a generation counter after the awaited StopRun; a refused or 501 update restores `prev` config and bumps the revision. Two new self-test cases.

### U62 session turns through admission; eviction keeps the queue head
**Landed** as 2896377; seven preemption tests. Not done: tracing samples for session turns. Note for the brief record: `deadline_aware` estimates by batch slot, so the test lowers `max_batch` to 4 to make the spiral shed.
**Spawned** on `claude/tl-session-admission`, model default. From R2 on U22: follow-up turns go through `dispatch()`'s admission (routing stays pinned) and count in `first_attempts`; decode-growth eviction passes `keep = Some(head)`. Only the `11-preemption` golden rows may move; the report carries the corrected demo-11 numbers for finding 8.
### U63 trace workload fixes from R3 (`model: sonnet`)
**Spawned** on `claude/tl-trace-fixes`: stable sort of trace rows before taking the origin; `Workload::offered_rps(sc, window)` returns rows-in-window ÷ interval in trace mode, used at the frame build; two tests.

### U64 walkthrough runner fixes from R3 (`model: sonnet`)
**Spawned** on `claude/tl-runner-fixes`: `skip()` settles only when the cursor has arrived; a cursor already past the step rewinds on replay and explains on live; a run ending before the step settles with a reason; three self-test cases.

### U65 trace recorder bounded snapshots (`model: sonnet`)
**Spawned** on `claude/tl-trace-growth`: `take` drops snapshots below the smallest still-referenced step index and rebases; spans byte-identical; one test over 10,000 steps.

### U66 golden file merges by union; html_md5 without the scenario block (fix-once from the productivity agent, `model: sonnet`)
**Spawned** on `claude/tl-golden-union`: `.gitattributes` with `merge=union` for `bench/golden-fingerprints.txt` and a new `bench/fingerprint-runs.txt` (the run list out of check-fingerprints.sh, name-keyed); html_md5 hashed over the report with its embedded scenario text stripped, golden refreshed once with every fingerprint/events/summary row unchanged.

### U68 integrate.sh: md-only diffs skip the gate (fix-once from the productivity agent, `model: sonnet`)
**Spawned** on `claude/tl-integrate-md`: when `git diff --name-only origin/master...HEAD` is all `*.md`, stages 3 and 4 are skipped with a line saying so; measured cost was 57–92 s per graph update and 59–68 s lock waits for code units behind it.

### U69 trace recorder: eviction span and warmup quotas (queued, low)
R3 unverified: after a preemption without swap `prefill_left = resident`, so re-prefill chunks are recorded again with no span marking the eviction; per-window sampler quotas are consumed by warmup completions before the `measured_from` filter. Specify once U62 settles preemption's semantics.
### U26b demo 12 speculative decoding and its walkthrough (`model: sonnet`)
**Spawned** on `claude/tl-spec-demo`: run-demos.sh block 12, `Demo { group: "12-spec-decode" }`, counts 40 → 42, `spec-decode.json` with `run`/`compare` and the finding's numbers, index card, self-test ids. Makes the selected set 12 when it lands.

### U70 mock / replay / live labels on every page (`model: sonnet`, Issao's stop request)
**Spawned** on `claude/tl-data-labels`: `mode.ts` banners and `DATA_SOURCE_LABEL`/`dataSourceGloss`; `Panel` tags carry the word for the panel's source (mock when any read field is unwired); Home cards and report links labelled (reports: "real engine output, a replay, not live"); Dashboard/StatusBar/LoadTest header; Compare both sides; Showcase header "stepping through a replay" / "driving a live run" and `not applied (replay):` / `not applied (live, refused):` prefixes. Self-test cases for label and gloss per mode; bundle grep; headless Chromium pass if available.
### U73 tools/qa: the headless Chromium gate (default model)
**Landed** as 8c75b9f (21:38); harness output in the status line above; the two findings became U76.
**Spawned 21:27** on `claude/tl-qa-harness`, port 8181. Main's `qa.js`/`qa2.js`/`sc.js` from `/home/agents/.claude/jobs/b22c4410/tmp/pw` become `tools/qa/qa.js` (BASE from `QA_BASE`, default `http://localhost:8181`) plus `tools/qa/serve-local.sh` (release build through tools/build.sh, `npm run build`, `sim-run export --demos --dir web/dist`, reports copied from run-demos.sh's `out/` unless `QA_SKIP_REPORTS=1`, `PORT=<port> sim-run serve --dir web/dist` in the background, wait for ListRuns, run qa.js, kill the server). Checks written against the target state of this session, so they fail until U74/U75/U72/U71 land: dashboard live and advancing at 1× with no "speed 0×"; every showcase card opens and its run advances; the nav link returns to the cards; a true fresh load of `#/showcase` (a new page, not a hash change) shows cards and issues no StartRun; demos 11 and 12 send their engine keys in the StartRun body; Home's twelve report links answer 200; no JS errors, no 4xx/5xx.

### U74 the live dashboard starts paced (default model)
**Landed** as a5061da (21:32); 10/10 server self-test; cursor advancing at 1× in headless Chromium.
**Spawned 21:27** on `claude/tl-paced-start`, port 8182. `ServerRunEngine.start(play)` sends `maxRealtimeFactor: this.lastFactor` on both paths (the wire's 0-means-unpaced semantics untouched; the client's default changes), `speed` reads `lastFactor` when the status is unknown or reports 0, and an exported `speedLabel(status, paused)` ("paused", "N×", "unpaced") replaces the `speed {factor}×` fragment in `ServerBanner`. Self-test cases in server.selftest.ts against the fake ingress; browser check with the harness.

### U75 Showcase: the open walkthrough lives in the URL; the source comes from the probe (default model)
**Landed** as de7536f (21:39).
**Spawned 21:27** on `claude/tl-showcase-url`, port 8183. `#/showcase?script=<id>`: `App.currentRoute` strips the query; Showcase derives the open script from the hash and follows `hashchange`, a card sets the hash, exit clears it, so the nav link, the back button and a fresh load all show the cards. `initialSource` asks the dashboard's one-per-page probe (exported from useRun.ts) instead of `serverMode().enabled`, and passes `data="server"` when it answers server, so the narration word matches the run. Reports what, if anything, can open a run at `#/showcase` without a click.

### U72 engine keys ride on `ScenarioConfig.extra` (default model; was queued as sonnet, grew a design)
**Landed** as 77fb48f (21:36); five self-test cases; one obsolete replay assertion (`admission reported as unmapped`) rewritten.
**Spawned 21:27** on `claude/tl-extra-keys`, port 8184. `extra: Record<string, number | string>` on `ScenarioConfig` for every key `Scenario::parse` accepts that has no control-panel field; `scenarioConfigToWire` sends them at StartRun; `configFromScenarioText` fills them instead of listing them unmapped; `applyPatch` routes a one-segment unknown path there; `kv-spiral.json`'s scenario becomes `kv_spiral_never.txt` key for key. Round-trip cases in replay.selftest.ts and walkthrough.selftest.ts.

### U78 `least_kv_probe` as a `RoutingKind`; favicon link (`model: sonnet`)
**Landed** as a73cb7c (22:05); 19/19 walkthrough self-test; the card live in the gate.
**Spawned 21:55** on `claude/tl-probe-kind` from the gate's two residuals. `RoutingKind` gains `least_kv_probe` (`ROUTING_TO_ENGINE`, `ROUTING_LABEL`, `ROUTING_NOTE`, the mock engine's switch as a fall-through with `least_kv_tokens`); `<link rel="icon" href="data:,">` in web/index.html. One walkthrough self-test case; done when the gate shows that card live and Home with no console error.

### U76 live controls before the run id; StopRun on page close; harness cleanup (default model)
**Landed** as 945a727 (21:50).
**Spawned 21:38** on `claude/tl-pending-controls`, port 8186, from U73's gate output. `wantPaused` remembered by `setPaused`/`setSpeed`/`start`, one SetSpeed after StartRun when it differs from what StartRun was sent; `pagehide` → `dispose()` with a keepalive StopRun; qa.js posts StopRun for every run id it saw. Three self-test cases against the fake; done when the local gate shows live walkthroughs advancing with no 503.

### U71 banner assertions, CSS, Home reports 1–12 (`model: sonnet`)
**Landed** as f97c175 (21:38); 33/33 and 17/17 standalone.
**Spawned 21:27** on `claude/tl-web-small`, port 8185. api.selftest.ts cases 32/33 and replay.selftest.ts case 17 assert U70's wording; `.wt-mode` and `.tag-line` rules; Home lists reports 7–12 with their finding titles ("Six" becomes "Twelve").

### U77 live-run cap: busy or leased runs only; evict, then reap, idle-stopped runs (sim-ingress, default model)
**Landed 19:41** as f4a119f (0b31639). Test (a) leases the live runs by predicted id before starting them, since a 50 ms threshold leaves no window afterwards; a writer still holding an evicted run's `Arc` waits out its lease as before.
**Spawned 19:36** on `claude/tl-run-cap` after U79 landed; brief carries `start()`, the `RunState` fields, `drive()`'s idle block as U79 left it, the lexical-order trap on `r-N` keys, and the two tests with the lease-per-live-run trick for a 50 ms threshold.
Main's decision, 21:39, verbatim: *"a run counts toward the eight only while it is busy or leased (unpaced-running, stepping, or paced with a live lease). An idle-stopped run (checkpoint written, no lease) does not count; and when a StartRun arrives at the cap, the server evicts the oldest idle-stopped run (its checkpoint under runs/<id>/ stays, GetRun answers 404 after eviction, or 410 if you prefer to say "gone") before answering 503. Reap idle-stopped runs anyway after 2 × IDLE_SHUTDOWN_SECONDS so memory is bounded even with no new arrivals. Rationale: the cap exists to bound CPU and memory for live work; a parked checkpoint is neither, and a crashed tab must never be able to lock the public site. No proto change; document it in WIRE.md next to the idle rule."* Not spawned tonight: Issao's scope for this session is main's items 1–4 then stop, and tonight's deploy does not wait for it. Files: crates/sim-ingress/src/run.rs (the registry's count and `start()`'s cap check), server.rs (the 503 path), WIRE.md; tests: a cap test that starts nine runs with the first idle-stopped, a reap test over the run thread's clock.

### Queue at the stop, in the order the next tech lead should take it
0. **U77** (default): the live-run cap decision above.
1. ~~U72~~ spawned 21:27 as above. Original note: `web/src/lib/config.ts` and `replay.ts`'s `configFromScenarioText` map the preemption keys (`preemption`, `preemption_victim`, `session_turns_mean`, `session_think_s`, `dram_capacity_tokens`) and the speculation keys (`spec_draft_tokens`, `spec_accept_rate`) so `scenarioFor(script)` keeps them and demos 11 and 12 run live; moves the counts to 12/12/12.
2. ~~U71~~ spawned 21:27 as above. Original note: `web/src/lib/api.selftest.ts` and `replay.selftest.ts` still assert the pre-U70 banner strings and fail standalone (integrate.sh runs only `npm run build`); update the assertions; add `.wt-mode`/`.tag-line` rules to styles.css. Consider running the web self-tests inside integrate.sh's stage 5.
3. **U31b** (default): outlier ejection policy on the health vector (`ReplicaView.ejected` from a rule over `last_step_ns` vs the fleet median for m consecutive views, cooldown), scenarios `gray_failure_none.txt`/`gray_failure_eject.txt`, demo 13, walkthrough, finding 9 final numbers.
4. **U25b** (sonnet): per-class goodput/attainment rows in the report table and the export (`RunResult::classes`, `class_goodput_tokens_s`, `class_attainment` exist); U44's longer window for batch stays future work.
5. **S3** simplification pass over sim-ingress (grew by U18, U54, U55, U56, U59b), then sim-leaf; one at a time; `export.rs::override_key` delegates to `Scenario::with_override`.
6. **U69** (low): trace recorder eviction span and warmup quotas; U26's note that the pre-decode eviction check assumes one token per sequence per step at N>0.
7. Then the engine roadmap as before: U27 prefix caching, U30 tiering/disaggregation, U32 autoscaling and multi-geo (after U31b), U33 Bode, U29/U36/U37 the leaf split and scale validation, U38 shaping contrast report, U39 model weights.
Documented, not owed: the Cloud Run request-scoped-CPU note is in WIRE.md (U54); R3's findings are all landed (U63, U64, U65) except U69.

## Wave of 2026-09-07 (U79–U83), from Issao's morning instruction

### U79 the live server degrades on Cloud Run; request log; lock order (default model)
**Landed 19:36** as 60327b7 (0ddb039 the fix, 09118ae the request log). The pre-pause lbsim.ai log ended at 38 passed / 1 failed, the failure a harness-side page close. **Re-spawned 19:18** from bae16cc with the diagnosis below and the two WIP tests; scope is the hang, the lock order leases → subs → run.state, and the request log at `GET /requests.log`; U77 split out (see its section). Earlier resume note kept for the record: **Resume:** branch `claude/tl-cloud-hang`, worktree `/home/agents/repo/lbsim-wt-cloud-hang` (a WIP checkpoint on origin if the agent got that far; else the branch is at origin/master). At the pause the agent was running the lbsim.ai reproduction (`/tmp/u79-remote.log`; a local `sim-run serve` on 8191 and a `node tools/qa/qa.js` of its may still be up: `pgrep -af "sim-run serve|node tools/qa"`, kill by pid). Next step: the two failing tests in server.rs's test module, then the fix, then `QA_PORT=8191 QA_SKIP_REPORTS=1 tools/qa/serve-local.sh` three times, then integrate.
**Spawned 11:19** on `claude/tl-cloud-hang`, port 8191. Evidence and what was ruled out: docs/wrap-up-2026-09-06.md §5b. The
brief carries the diagnosis in the status line above and asks for the proof first: a test that opens a subscription on a run
stopped before its first closed frame and asserts the stream ends within 5 s on a thread with a timeout (it hangs on master),
and a test that a reconnect holding the final sequence ends the same way. Fix: `stream_updates` decides under the lock and
finishes outside it; `drive()` reads `live_for_run` before taking the run lock; the order leases → subs → run.state is written
down in WIRE.md. Request log: one stdout line per request (`req <method> <rpc|path> <status> <ms> [run=] [sub=]`) and per
stream (`sse open`/`sse end reason=`), kept in a 512-line ring served as text at `GET /requests.log`, because the deploy
account cannot read Cloud Logging. U77 (main's decision, quoted in its section below): `Registry::start` counts non-terminal,
non-idle-stopped runs; at the cap the oldest idle-stopped run is evicted (its checkpoint stays, `GetRun` answers 404); an
idle-stopped run is reaped after 2 × IDLE_SHUTDOWN_SECONDS regardless. Files: crates/sim-ingress/src/{server,run,lib}.rs,
WIRE.md. Done when the new tests pass, `QA_PORT=8191 tools/qa/serve-local.sh` passes three times in a row, and the report
carries the lbsim.ai harness output from before the fix; then READY TO DEPLOY goes to main and the harness runs against
lbsim.ai three times after the redeploy.

### U80 Home in three sections (`model: sonnet`)
**Landed 19:18** as 023672d (WIP checkpoint 50e4cf0 integrated as-is: build green, home checks pass). Earlier resume note kept for the record: **Resume:** branch `claude/tl-home-sections`, worktree `/home/agents/repo/lbsim-wt-home-sections`. Next step: finish Home.tsx per the brief, `(cd web && npm run build)`, `QA_PORT=8192 tools/qa/serve-local.sh` with reports, integrate.
**Spawned 11:19** on `claude/tl-home-sections`, port 8192. `web/src/pages/Home.tsx` only. Three `h2` headings, exactly
"Live", "Replay", "Mock", each with a one-line gloss; every link on Home in exactly one section. Live: the load-test
dashboard (`#/dashboard`, a run starts on the server when it opens) and the showcase (`#/showcase`, walkthroughs that drive
live runs). Replay: the dashboard in replay mode (`?server=off#/dashboard`) and the twelve reports (real runs rendered as
HTML). Mock: the A/B page (two mock runs, same seed) and the note that in live and replay a panel for a dynamic the engine
does not simulate yet carries a `mock` tag. Finished-product wording throughout: the "stand-in" lede and the "engine does
not exist yet" footnote go. Harness: the twelve report links still answer 200; U81's harness asserts the three headings.

### U81 the top-right badge is accurate per route and state (`model: sonnet`)
**Landed 19:35** as bd5ba3f (8e5c584 badge wiring, 0a09459 the `ui.tsx:59` grant plus the mount-order race fix). **Re-spawned 19:18** from da04703 (mode.ts done); brief carries App.tsx whole, Dashboard.tsx 51–73, the qa.js sections and the per-route badge regexes; card filter becomes `!e.disabled`. **Resume:** branch `claude/tl-mode-badge` at da04703 (WIP: only `web/src/lib/mode.ts` changed: `badgeText`/`badgeTitle` and the extended store), worktree `/home/agents/repo/lbsim-wt-mode-badge`. Next: App.tsx (publish `none`/`connecting` from the hash, render `badgeText`, brand line), Dashboard.tsx (`ServerDashboard` connecting/refused), Compare.tsx (`setActiveMode('mock')` on mount), qa.js (`badge()` helper, per-route assertions, `!e.disabled` card filter), then `QA_PORT=8193 QA_SKIP_REPORTS=1 tools/qa/serve-local.sh > /tmp/u81-qa.log 2>&1; grep FAIL /tmp/u81-qa.log`, integrate.
**Spawned 11:19** on `claude/tl-mode-badge`, port 8193. Files: `web/src/App.tsx`, `web/src/lib/mode.ts`,
`web/src/pages/Compare.tsx`, `web/src/pages/Dashboard.tsx` (the `Dashboard`, `ServerDashboard` and `ReplayDashboard`
functions only), `tools/qa/qa.js`. The badge names what is on screen now: nothing on Home and on the showcase card list;
"connecting…" while a dashboard probes or a live run has no id yet; `live — … · run r-N` once it does; `replay — …: <run>`;
`mock — …` on the A/B page (which never published its mode, so it inherited the last page's) and on a dashboard whose probe
failed; the server's refusal when StartRun was refused. The brand line stops saying "stand-in". The harness asserts the
badge text per route (`.mock-global`), Home's three headings, and picks scripted cards by `:not([disabled])` instead of the
"not scripted yet" text U82 removes.

### U82 build-process references out of user-facing text (`model: sonnet`)
**Landed 19:25** as 837910b (20 PASS locally, reports skipped; grep sweep clean; `kv-spiral.json:86`/`spec-decode.json:37` are "spending", not "pending", left alone). **Re-spawned 19:18** from a branch equal to master, with six edits: index.html title/description; Showcase.tsx intro (no repo paths, no schema link) and card foot (`coming`, `no walkthrough yet`), the line-175 note; StatusBar.tsx 34–35 and the line-9 comment; index.json note and the four `export … pending` requires; least-kv-probe.json:48 and retry-storm.json:34, then the grep over the fourteen scripts. Known interplay: its local gate may FAIL on the eight `coming` cards until U81's `!e.disabled` filter lands; told to integrate anyway if those are the only FAILs. **Resume:** branch `claude/tl-finished-text`, worktree `/home/agents/repo/lbsim-wt-finished-text`. Next step: the grep in the brief over index.html, Showcase.tsx (top function only), StatusBar.tsx, walkthroughs; reword; `npm run build`; `QA_PORT=8194 QA_SKIP_REPORTS=1 tools/qa/serve-local.sh`; integrate.
**Spawned 11:19** on `claude/tl-finished-text`, port 8194. Files: `web/index.html` (title, description),
`web/src/pages/Showcase.tsx` (the `Showcase` function only: intro paragraph, phase labels, card footers, the schema.md link),
`web/src/panels/StatusBar.tsx`, `web/public/walkthroughs/index.json` and the fourteen scripts (narration that names
exports, U-numbers, "recording" where the run may be live). Unscripted cards stay `disabled` with a plain "coming" foot.
Reports: grep `out/*.html` for build words and report, since sim-report is not owned. Keeps the mock / replay / live words.

### U83 UI pass (default model)
**Landed 19:49** as b5f5af7 (4c2c5f2). Reviewer (sonnet) spawned over the after-PNGs at the same time; its findings become units. Seen and left by U83, now U84 and U85 below.
**Spawned 19:35** on `claude/tl-ui-pass`, port 8195, after U81 landed. Brief carries PlaybackBar 56–98 and 165, RunTab, both banners verbatim, the handle fields, and qa.js's Playwright pattern; six decisions (rewind absent off-mock, RPC-name titles to words, Run tab playback section gone, one-line banners with `title` reasons, `dropped` fields disabled in place with a "server" note, `screens.js` before/after). The reviewer agent (sonnet) is spawned by the tech lead over the after-screenshots once U83 lands; its findings become a unit.
Files: `web/src/components/PlaybackBar.tsx`, `web/src/panels/ControlPanel.tsx`, `web/src/pages/Dashboard.tsx` (banners),
`web/src/styles.css`, a new `tools/qa/screens.js`. Known items: the live banner's "inert controls: fleet.accelerator,
workload.perturbation…" and "not served by this server, panels stay mock: …" lists go, and the controls they name are
disabled with a one-word reason where they are shown; the Run tab's second speed selector and play/step/rewind buttons go;
the playback bar reads play / speed / position, rewind hidden where rewind is off, the "mock run" tag gone (the header says
the mode); the two "stand-in" notes in ControlPanel reworded; consistent spacing; the replay picker and the live banner never
both show. Done when a reviewer agent (sonnet) walks `screens.js`'s screenshot of every route and tab and finds nothing to
remove.

### U84+U85 the Control panel header tag follows the mode; replay disables the load, policies and cluster tabs (`model: sonnet`)
**Landed 20:01** as 90e6b93 (ab65627); nothing skipped, `Panel` needed no change.
**Spawned 19:55** on `claude/tl-panel-mode` at main's instruction (main's "U87"). Main, verbatim: *"(1) the Control panel header's MOCK tag must reflect the mode: live → "live" (changes go to the server), replay → "replay", mock → "mock"; (2) on replay, the load and policy tabs are disabled with the one-word reason "recording" (a recording cannot change), sliders not rendered as active."*
From U83: `Panel` in `web/src/components/ui.tsx` tags the Control panel `MOCK` in every mode, including live, where load and policy changes go to the server. The tag should be the page's source word, or absent, for a panel that is controls rather than data. One condition; harness asserts no `mock` tag on the control panel in live.

### U85 (folded into U84 above)
From U83: on replay every load and policy slider looks active although `UpdateWorkload`/`UpdatePolicies` are refused with a reason. Issao's instruction ("no knobs that are not doing anything") says disable them; the alternative is a one-line notice at the top of the two tabs. Default if nobody says otherwise: `<fieldset disabled>` around the two tabs with the reason as the note, the same wrapper U83 built for `dropped`.

### U86 the first sample never arrives on about one showcase card in fourteen; a poller outlives its page (default model)
**Landed 20:01** as 9c22131 (07c93c1, d4d2917, d1d6dc7). Brief gaps it named: `FetchLike` in api.ts, the `overlay` placement at Dashboard.tsx:317, `setPaused`'s body past line 400.
**Spawned 19:51** on `claude/tl-first-sample`, from main's two residuals after lbsim-00014-gvp. Brief carries `start()` verbatim, `onRun`, `DashboardBody`'s waiting branch, qa.js section d, the six-run evidence, and the two tests to write first (a fake Ingress answering SetSpeed in reverse order; dispose during the SetSpeed await, then zero GetRun over three poll intervals).

### U87+U88 internal names out of the control panel and status bar; readouts for view-only values (`model: sonnet`)
**Landed 20:24** as 03bd74e (b598c0d, bba5796); recorded from git and main's report, its own report having been sent after its gate finished.
**Spawned 19:55** on `claude/tl-panel-text`, owning the tab bodies, StatusBar's live line, `Slider`, styles.css.
From the U83 review. `web/src/panels/ControlPanel.tsx` (Load tab foot, Policies tab's `PolicySpec` and `required_resimulation` sentences, Cluster tab's `bench/validate_epochs.py`), `web/src/panels/StatusBar.tsx` (the subscription-count sentence into a `title`), `web/src/styles.css` if the Load foot clipping is a layout rule. Plain facts, no type names, no repo paths.

### U88 (folded into U87 above)
From the U83 review: the SLO targets on the Policies tab and the sample rate on the Run tab are marked "view only" but draw a drag handle. Render them as `<dl>` readouts (or a disabled `Slider` variant with no handle), same note text. `web/src/panels/ControlPanel.tsx`, `web/src/components/ui.tsx` only if `Slider` needs a `readonly` prop.

### U89 Showcase and A/B leftovers (`model: sonnet`)
**Landed 20:10** as a9252da. Left unconditional, per the narrow ask: the pager's "N replicas exist…" grow note.
**Spawned 19:55** on `claude/tl-showcase-tidy`.
From the U83 review: `.card-foot`'s `needs:` span truncates mid-word (add `text-overflow: ellipsis`, it is already `nowrap`; or wrap two lines); the walkthrough note "compare …: this page has no A/B view yet, so the second run is not opened" becomes a plain fact without "yet"; the replica table hides its pager and row-size picker when it has no rows; the A/B page's "prove the refusal" checkbox goes. `web/src/pages/Showcase.tsx`, `web/src/pages/Compare.tsx`, `web/src/panels/` the replica table file, `web/src/styles.css`.

## Wave of 2026-09-07 evening (U90–U95), from Issao's instructions via main

Issao, verbatim: *"the machine page always shows 0 replicas, looks like a bug. also, the replica slider maxes out at 128, let's increase that to at least 10k, even if simulation speed may fall behind. also, make the default for most scenarios a bit larger for realism, say 256 replicas."* Then: *"I would like to see gpu utilization in the utilization page, and show percentile graphs for utilization too."* Then: *"in the dynamic 1 of showcase, I see live in the top right corner but the pop up explaining says it is mock data, which one is it?"*

### U90 the Machines page shows 0 replicas on live (default model)
**Landed 21:13** as 258c706: `useServerReplicas(runId, ids, rate)` in useServerRun.ts, one SCOPE_REPLICA subscription per visible row, history capped at 240 per id; MachineLevel's `live` prop; qa.js `machines()` helper asserting on live, replay and mock (`?server=off&replay=off#/dashboard`). Before: live FAIL, replay/mock PASS; after: all three pass. Left: the live heatmap aligns columns to the last N fleet samples by count, not by timestamp. Brief gap: from a worktree the harness needs `NODE_PATH=/home/agents/.local/opt/node/lib/node_modules`.
**Spawned 21:00** on `claude/tl-machines-live`, port 8195. Diagnosis in the brief: `useServerRun.ts:403` builds live frames with `frameFromUpdate(u, t0, tick)` and no replica rows, so `frame.replicas` is `[]`; `MachineLevel` takes its id list from `frame.replicas`, finds none, opens no per-replica subscriptions (and the ones it "opens" go to the stand-in `subscriptions.ts` registry, which never touches the wire). Decision: on live the id list is `0..readyReplicas-1` from the fleet row, one real `SCOPE_REPLICA` subscription per visible row through a new `useServerReplicas` hook in `useServerRun.ts` (bounded to the page, `subscribeToTarget` + `replicaFromUpdate`), heatmap history from the same page-bounded stream; replay and mock unchanged in mechanism but verified. No proto change: a per-replica repeated field in the fleet row was considered and not needed. Owns `web/src/panels/observe/MachineLevel.tsx`, `web/src/lib/useServerRun.ts`, `web/src/panels/ObservationPanel.tsx`, `tools/qa/qa.js` (section b, a Machines-tab assertion; and the replay/mock counterparts). Done when qa.js asserts a non-zero replica count and ≥1 row on the Machines view in live, replay and mock.

### U91 replica slider to 10,000; the banner says the pace actually achieved (`model: sonnet`)
**Landed 21:13** as c9d7c0d: slider `max=10000`; `ServerRunEngine.achievedFactor` from the last ~2 s of fleet updates; `speedLabel(status, paused, achievedFactor)` prints `2× (achieving 1.3×)` under 90 % of target. A 10,000-replica run started locally and streamed `"60":10000` rows at factor 1. No server cap needed raising (validate allows 200k, body cap untouched by fleet size). Brief gap: OpenSubscription is a GET with query params, not a POST.
**Spawned 21:00** on `claude/tl-replicas-10k`, port 8196. `ControlPanel.tsx:431` `max={128}` → 10000; `sim-leaf::validate` already allows 1..=200000 and `MAX_BODY_BYTES` 1 MiB is untouched by fleet size (checked, reported); the achieved factor is measured client-side in `ServerEngine` from sim-time advanced over wall time, and `speedLabel` appends "achieving N×" when it falls under 90 % of the target. Owns `web/src/panels/ControlPanel.tsx`, `web/src/lib/useServerRun.ts` (the `speedLabel` and `ServerEngine.onUpdate` hunks only; U90 edits the same file elsewhere, keep both on rebase), `web/src/lib/server.selftest.ts`. Done when a 10,000-replica StartRun against a local `sim-run serve` streams samples and the banner shows the achieved factor.

### U92 default fleet 256 replicas at the same offered/capacity ratio (default model)
**Landed 21:18** as ba009c7. Realtime factor of base at 256 replicas on one core: ~180–235× (0.5–0.7 s wall for 120 s of sim, 2.39 M events). 50 golden rows moved, holdouts and the kept demos byte-identical. Kept small with a comment: the KV spiral pair (4), the fair-share tenant pair, the no-decode pair, trace_replay (32). Tests re-pinned where they literally encoded 32/70/40; `tests/common/mod.rs::small()` pins 20 rps so trace_workload's fingerprint did not move. Also edited outside its list, flagged: `bench/fingerprint-runs.txt` (demo 4's sweep). Findings: every shape holds except finding 2 (staleness), where least_requests at 256 replicas collapses at every interval (SLO 0.79→0.15 at 100 ms, 0.39→0.045 at 1000 ms, CV up to 5.5, queue-cap rejections at 2–4 s); main decides whether demo 2 keeps 32 replicas or the text is rewritten. Table: `/tmp/u92-before-after-compact.txt` and main's inbox.
**Spawned 21:00** on `claude/tl-default-256`. `scenarios/base.txt` and every demo scenario at 32 replicas go to 256 with `arrival_rps` ×8 (70→560, 237→1896, 120→960); demo 4's sweep values ×8; kept small and said so in the scenario comment: the KV spiral pair (4 replicas), the fair-share tenant pair, the no-decode pair, `trace_replay.txt`. `Scenario::default()` and `web/src/lib/config.ts` `DEFAULT` follow. Deliberate behaviour change: `./run-demos.sh` before and after, `bench/golden-fingerprints.txt` re-baselined in the same commit, before/after numbers per finding reported (main decides on findings text; the unit does not touch `docs/findings.md`), realtime factor of the 256-replica base measured on one core. Owns `scenarios/*.txt`, `run-demos.sh`, `check-sensitivity.sh` (if it carries `arrival_rps` literals), `bench/golden-fingerprints.txt`, `crates/sim-scenario/src/lib.rs` (the `Default` hunk), `web/src/lib/config.ts`, plus any test under `tests/` whose expectation names 32 replicas.

### U93 qa.js waits for the recording before the two replay asserts (`model: sonnet`, tiny)
**Landed 21:13** as fb9972b. Against lbsim.ai: 57/2 before, 59/0 after.
**Spawned 21:00** on `claude/tl-qa-replay-wait`, port 8199. The two false fails on lbsim.ai (§5c of the wrap-up). Owns `tools/qa/qa.js` section f only.

### U94 GPU utilization, per replica and as percentiles across the fleet (main's instruction; split a/b/c)
Proto on master 6a7453b: `METRIC_GPU_UTILIZATION = 67` (busy share of the sample window per replica; at fleet scope the mean in `values` and a distribution across replicas in `distributions`), `METRIC_GPU_COMPUTE_BOUND_FRACTION = 68` (share of busy time under the compute roofline).
- **U94a engine (default)**, **spawned 21:00** on `claude/tl-gpu-engine`. `CostModel::step_split` names the compute-bound part of a step (prefill and verify terms) beside the bandwidth-bound part (fixed weight read, per-sequence, KV re-read); `Replica` accumulates busy and compute nanoseconds, clipped at the sample instant so a step across a boundary is split exactly; `sim_metrics::ReplicaSample` gains `busy_ns` and `compute_ns` for the window `(previous sample, t]`. Fingerprints unchanged (no event or summary change). Owns `crates/sim-physics/src/lib.rs`, `crates/sim-model/src/lib.rs`, `crates/sim-leaf/src/lib.rs`, `crates/sim-metrics/src/lib.rs`, `tests/frames.rs`.
- **U94a landed 21:12** as 20cb4d9: `ReplicaSample.busy_ns`/`compute_ns` per window, `Replica::busy_ns_through(t)`/`compute_ns_through(t)`, `CostModel::step_split`, `Window.prev_busy/prev_compute`; fingerprints unchanged; a crashed replica's in-flight step counts to its scheduled end (one step of overhang), swap transfer time is busy but not compute.
- **U94b landed 21:35** as 9d4aa1b: consts 67/68, `distribution_over_replicas` (exact 50/90/99 across replicas) for GPU and KV at fleet scope in run.rs and export.rs, replica scalars; golden `summary_md5` re-baselined for the `gpu_utilization_mean` row.
- **U94b wire (sonnet)**, **spawned 21:12** on `claude/tl-gpu-wire`: `wire.rs` table + consts, `run.rs` `FLEET_METRICS`/`REPLICA_METRICS`/`row()`, `export.rs` fleet and replica rows, a `distribution_over_replicas` helper for exact percentiles 50/90/99 across replicas (also emitted for `METRIC_KV_UTILIZATION` at fleet scope, so the KV chart gets the same bands; a wire-contract addition reported to main), `summary.csv` gains `gpu_utilization_mean` → fingerprints re-baselined in the same commit.
- **U94c landed 21:20** as 64d9ad0 (`Frame.gpuUtilization/gpuComputeBoundFraction/gpuUtilizationP/kvUtilizationP`, `ReplicaSample.gpuUtilization/gpuComputeBoundFraction`, `derive.percentilesOver`, `Metric` 67/68; MachineLevel's `pendingRow` and types.ts folded in after a stage-5 refusal). qa.js section g: the panel-exists checks pass; the two non-zero-mean checks fail until U94b lands, by design.
- **U94c web (default)**, **spawned 21:00** on `claude/tl-gpu-web`, port 8197, against the wire shape fixed in prose above (stand-in: fixture rows in `apiFixtures.ts`). Utilization page: GPU utilization chart with the fleet mean and p50/p90/p99 bands over time, KV chart with the same bands, wasted-GPU note stays; mock keeps invented numbers under its tag; `wired.ts` lists the new fields; qa.js asserts the GPU panel renders with a non-zero mean on live and replay (green once U94b is on master).

### U95 a partial panel on a live or replay run says the mode word, with the exception in plain words (`model: sonnet`)
**Landed 21:20** as d01f991.
**Spawned 21:00** on `claude/tl-partial-tag`, port 8198. `panelTagWord` returns the mode word for `partial` on a wire frame; `MockTag`'s title and a small inline note read e.g. "live — 5 columns not simulated yet, shown as placeholders: replica state, prefix hit rate, TTFT mean, speed multiplier, weight", field names through a human-label table in `wired.ts`; the bare word "mock" only when the whole panel is invented. Owns `web/src/lib/wired.ts`, `web/src/lib/wired.selftest.ts`, `web/src/components/ui.tsx`, `web/src/styles.css`, `tools/qa/qa.js` (section b: a tab walk asserting no `.mock-tag` reads "mock" on the live dashboard unless its title starts with "live —").

### U96 the A/B page runs live and replay, not only mock (default model)
**Landed 21:21** as 53c5993: `ServerCompare` (two `useServerRun` handles, same seed, routing differs; shared controls fan out; cursor = min of the two live edges), `ReplayCompare` (`?runs=a,b` or the first walkthrough with `run`+`compare`), `MockCompare` unchanged; Home lists A/B under Live. Harness: r-17 vs r-18, both seeds 20260906, cursor advances, difference +7.5 %, badge live. Seed comes from `a.config.seed` (the page's own StartRun bodies; `RunStatus` carries no seed — a proto gap for main). No hook change was needed. The Difference panel is partial on live (`wastedGpuFraction` unwired), so it shows U95's `live —` note.
**Spawned 21:07** on `claude/tl-ab-live`, port 8200. Issao: *"what is missing for a/b to work with the live simulator?"* Main's decision: three-mode like Dashboard. Live = two `useServerRun` handles from one scenario and seed differing only in the policy, two subscriptions, one shared cursor equal to the earlier of the two live edges, shared play/pause/speed (two SetSpeeds), a mid-run policy change (two UpdatePolicies), rewind disabled; replay = two `useReplayRun` handles from a walkthrough's `run`/`compare` pair; mock stays as the fallback under its tag. Tagline reads the server's seed; badge and panel tags follow the mode; Home moves A/B to the Live section. Owns `web/src/pages/Compare.tsx`, `web/src/pages/Home.tsx`, `tools/qa/qa.js` (the A/B block), not `useServerRun.ts` (U90/U91 are in it; the brief tells it to wait for both before touching it, and to report the change instead if they have not landed). Harness: on live, two runs streaming with equal seeds, the cursor advances, the difference panel is non-zero.

### U92b demo 2 back to 32 replicas (`model: sonnet`)
**Landed 21:32** as ff4e0ca: `scenarios/staleness_least_requests.txt` (32 replicas, 70 rps); the six `2-staleness/` golden rows are byte-identical to pre-U92, run ids unchanged so `stale-telemetry.json` needs no edit.
**Spawned 21:20** on `claude/tl-staleness-32`. Main: *"Demo 2 (staleness sweep) goes back to 32 replicas with a scenario comment saying why: at 256 even 100 ms is past the cliff, so the sweep no longer shows the curve that is the finding."* A new `scenarios/staleness_least_requests.txt` (32 replicas, 70 rps) so demo 1's least_requests leg stays at the 256 default; run-demos.sh, bench/fingerprint-runs.txt and export.rs `DEMOS` follow; its golden rows re-baselined (expected byte-identical to pre-U92).

### U97 the event ceiling scales with the run (`model: sonnet`)
**Landed 21:32** as 2eb5d4f with the original scope: `event_ceiling(sc) = max(50 M, peak_rps × duration_s × 40 + replicas × duration_s × 200)`, an 8 GiB RSS guard over `/proc/self/statm`, three tests (a 10k-replica 3 s run completes in debug). Superseded by U97b below, per Issao.
**Spawned 21:20** on `claude/tl-event-ceiling`. `MAX_EVENTS = 50 M` becomes `max(50 M, arrival_rps × duration_s × 40 + replicas × duration_s × 200)`; `MAX_IN_FLIGHT`/`MAX_RECORDS`/`MAX_QUEUE_LEN` stay; an RSS guard over `/proc/self/statm` if it fits in 30 lines. Tests: a 10k-replica scenario validates and a short one runs. Fingerprints unchanged.

### U98 herding's staleness cliff moves left as the fleet grows (queued, default model)
From U92's before/after: least_requests at 100 ms staleness, SLO 0.79 at 32 replicas and 0.15 at 256. A sweep over replicas (32, 64, 128, 256, 512) at fixed 250 ms telemetry with least_requests and arrival scaled to the same offered/capacity ratio, demo 13, report, recording, walkthrough; finding 10's text goes to main's docs agent.

### U97b no event ceiling; a memory budget from the environment (`model: sonnet`)
**Landed 21:54** as 925f758: `memory_budget_mb()` from `LBSIM_MEMORY_BUDGET_MB` (default 20000), checked every 1 M events over `/proc/self/statm`; `max_in_flight`/`max_queue_len = max(floor, replicas × duration_s × 20)`, `max_records = max(floor, peak_rps × duration_s × max_attempts × 2)`; the 10k-replica / 25k rps / 120 s run: 71.5 s wall, 916 MB peak RSS. deploy.sh sets 1600 MB for the 2 GiB container (main, 661cc65).
**Spawned 21:32** on `claude/tl-memory-budget`. Issao: *"is that just for running locally? If so, remove that when running with a cloud backend."* Main: remove the event ceiling entirely; abort when RSS exceeds `LBSIM_MEMORY_BUDGET_MB` (default 20000; deploy sets ~80 % of the container); `MAX_IN_FLIGHT`/`MAX_QUEUE_LEN` → `max(floor, replicas × duration_s × 20)`, `MAX_RECORDS` → `max(floor, peak_rps × duration_s × max_attempts × 2)`. A 10k-replica 120 s run completes locally; wall time and peak RSS reported.

### U99 cards never change height (`model: sonnet`)
**Spawned 21:32** on `claude/tl-no-jump`, port 8203. Issao: *"some of the boxes are jumpy because text is adding height to the cells (e.g. service quality> headline > wasted)..."* Reserved line heights on tiles, panel headers, banner and status bar; intermittent lines stay mounted with `visibility` and ellipsis + hover; tabular figures. Done when qa.js measures every card's box three times five seconds apart on the live dashboard and the showcase and they are identical; the wasted tile is the named case; screens.js gets the same three-moment mode.

### U98 herding's staleness cliff moves left as the fleet grows (lane A, default model)
**Landed 21:51** as d94fbf0. Five points at 0.30 offered/capacity, 250 ms telemetry, least_requests: SLO 79.5 / 56.5 / 29.5 / 14.5 / 7.0 % for 32 / 64 / 128 / 256 / 512 replicas; CV 0.71 → 2.03; TTFT p99 8.0 → 50.5 s; running timeouts 3 → 4,050; the 32-point's fingerprint equals demo 2's 250 ms row. Finding 10 paragraph in its report (forwarded with the READY). Brief gaps: sweeps write no summary.csv (only the HTML), and a rebase across another re-baseline leaves duplicate golden labels under `merge=union` (a fresh `--update` cures it).
**Spawned 21:32** on `claude/tl-herd-fleet`, port 8201. New key `arrival_rps_per_replica` so a sweep over `replicas` keeps the offered/capacity ratio; `scenarios/herd_fleet.txt` (least_requests, 250 ms telemetry); demo 13 `sweep --over replicas=32,64,128,256,512`; walkthrough `herd-fleet-size.json`, card, Home link, counts; finding paragraph to main.

### U31b outlier ejection over the delayed health view; demo 15 (lane A, default model)
**Spawned 21:51** on `claude/tl-ejection`, port 8205. A third pluggable kind, `HealthPolicy` (`none` | `outlier`), assessed once per telemetry delivery in the leaf and written into the same delayed `ReplicaView.ejected` every router and admission policy already reads; `outlier` ejects a replica whose `last_step_ns` exceeds `ejection_ratio` × the fleet median for `ejection_views` consecutive views, for `ejection_cooldown_s` (median over a 256-view stride sample above 256 replicas, so it is bounded). Scenarios `gray_failure_none`/`gray_failure_eject` (256 replicas, slow=0.3 at t=60); demo 15; `gray-failure.json` gets `run`/`compare` and measured numbers; finding paragraph to main. `ejection = none` keeps every run byte-identical.

### U27a prefix caching: tree, replica cache, session forks, the router seam (lane B, default model)
**Landed 21:54** as 361d59a, seam exactly as specified; `PrefixTree` lives in sim-workload (re-exported by sim-model); two streams `prefix`/`prefix_tree`; the cache has its own budget (not competing with live KV yet); merge-back out of scope. Edited four full-literal tests outside its list unavoidably; `tests/scenario_parse.rs`'s KEYS is hand-counted (65 now).
**Spawned 21:32** on `claude/tl-prefix-engine`. Keys `prefix_roots`, `prefix_root_tokens`, `prefix_zipf_s`, `session_fork_rate`, `prefix_cache_tokens`, `affinity_max_load_ratio`, `affinity_fallback_choices`, all default-off; `PrefixTree` in sim-model; `Request.prefix_node/prefix_tokens`; per-replica prefix-closed LRU cache over a `BTreeMap` (deterministic), hit discounts `prefill_left` (never adds to the parked-context discount); the leaf's `PrefixHolders` index (O(depth), never a fleet scan); seam in sim-policy: `RequestView.prefix_node/prefix_tokens`, `trait PrefixIndex`, `NoPrefixIndex`, `RouteContext::new(.., prefix)`; `ReplicaSample.prefix_hit_tokens/prompt_tokens`. Merge-back out of scope. Fingerprints unchanged.

### U27b `prefix_affinity` policy and demo 14 (lane B, default model)
**Spawned 21:32** on `claude/tl-prefix-policy`, port 8202, against U27a's seam in prose; non-engine work first, then rebases when U27a lands. Policy: best holder under `affinity_max_load_ratio × load estimate` (bounded-cost estimate, no fleet scan), else power-of-`affinity_fallback_choices`; scenarios `affinity_off/spread/sticky` (Zipf over 100 roots, fork rate 0.1, 400k-token cache, [GUESS] per calibration §9.1); demo 14; `affinity-vs-spread.json` gets `run`/`compare` and measured numbers; finding paragraph to main.

### U27c prefix hit rate on the wire and in the report; web keys (queued, `model: sonnet`)
After U27b: `METRIC_PREFIX_HIT_RATE` (48) at fleet and replica scope in run.rs and export.rs from `ReplicaSample.prefix_hit_tokens/prompt_tokens`; a report row and summary.csv row only when `prefix_roots > 0` (so no existing md5 moves); `config.ts`/`replay.ts` map the seven keys; the walkthrough quotes the hit rate.

### U95b no invented number on a live or replay run (default model)
**Landed 21:48** as e88b73c. Hidden on live/replay: the five MachineLevel columns, ClusterHealth's warming/draining/ejected tiles and series and the failure-events list, Utilization's wasted-GPU/preemptions/prefix-hit tiles and the tier charts, the whole Traces tab (synthesized from the seed; U104 makes it real). Harness 59/14 after (the 14 are report 404s under QA_SKIP_REPORTS and U94's two GPU checks, green once U94b is in the served build).
**Spawned 21:35** on `claude/tl-no-invented`, port 8204. Main: the five per-replica columns the engine does not produce (replica state, prefix hit rate, per-replica TTFT mean, true speed multiplier, routing weight) render "—" with the hover "not simulated yet", any other partial field likewise; the tag reads the mode word with no exception text; a chart that needs only invented fields is hidden on live/replay. Harness: zero `mock` tags and zero placeholder values on the live dashboard and a live showcase run. Superseded in part by U100 (the tags go away entirely); its "—" rule is what U100 folds in.

### U101 replica state, true speed and per-replica TTFT on the wire (`model: sonnet`)
**Spawned 21:35** on `claude/tl-replica-state`. `Replica::state()` (1 READY, 2 DEGRADED when speed < 1, 3 EJECTED when down), `speed()`, two TTFT counters per window; `ReplicaSample.state/speed/ttft_sum_ns/ttft_count`; replica rows carry `METRIC_REPLICA_STATE` 69, `METRIC_TRUE_SPEED_MULTIPLIER` 70 (fleet mean at fleet scope) and `METRIC_TTFT` as a count+mean Distribution; no proto change beyond main's 21de1ba; fingerprints unchanged. The web mapping of the three (state, speed, TTFT columns stop being dashes) is a follow-up inside U100.

### U100 the mock is gone: live and replay only (default model, queued behind U95b and U99)
Issao, verbatim: *"yeah, let's remove all invented numbers everywhere, every reference to mock in the ui, every link for a mock data view. it is not needed anymore."* Supersedes Home's Mock section (U80), the mock tags (U70/U84), the mock fallback in Dashboard/Showcase/Compare and U95's wording. Delete `MockEngine` (engine.ts), `rng.ts` if only the mock used it, the `subscriptions.ts` mock registry and its status-bar readout, the walkthrough mock branch, the `?server=`/`replay=` overrides that force mock, the `mock` data mode, `MockTag` and its gloss, Home's Mock section, the README's mock wording, the App title line if it still says stand-in. Modes: live when the server answers, replay of the recordings otherwise, else "no server and no recordings served". Unwired fields render "—" (hover "not simulated yet"); a chart that depended only on invented fields is removed. Per-panel tags go; the top-right badge is the one word. Done when `grep -ri mock web/src` returns only the "—" logic, the bundle has no "mock", qa.js's mock assertions are deleted (replay keeps `?server=off`), every route passes locally, and the walkthrough self-tests pass. Also maps U101's three wire fields into the replica table.

### U102 the walkthrough card's Play button (queued behind U100; sonnet if the runner already exposes the action, default otherwise)
Issao, verbatim: *"for the showcase card, include a 'play' button there that resumes the scenario at the predetermined speed, the card should show 'advancing scenario' until the next stop point shows up. Using the play button in the play bar should have the same effect."* Main: the narration card (step N of M) gets Play; pressing it resumes at the segment's `speed` (or the walkthrough default), the body reads "advancing scenario…" with the next stop's sim time until the runner reaches it and swaps in the next narration, the button disabled while advancing; the playback bar's play calls the same runner action (one function, two buttons); pausing from either place shows the card paused at the current step. Harness: on a live walkthrough press the card's Play, assert "advancing scenario" then the next title within the segment's duration at that speed; then the same via the bar.

### U103 the walkthrough card is draggable (`model: sonnet`, with or after U102)
Issao, verbatim: *"can you make the showcase card draggable?"* Main: drag by its header with pointer events (touch too, not the HTML5 drag API), stays where dropped across steps, clamped to the viewport, position remembered per browser in localStorage (try/catch), a small "reset position" affordance in the header; nothing else changes. Harness: drag 200 px, the bounding box moves by that amount and does not move when the next narration swaps in.

### U104 real traces on the Traces tab; GetTraces implemented (default model, after U100)
From main's vision-progress refresh (b0de727/ac371f4): the Traces tab reads `web/src/lib/traces.ts`, which invents traces from the seed, while `GetTraces` answers 501 (server.rs:315) although the engine records spans (U24) and the export carries them (U19). Under Issao's "remove all invented numbers everywhere": implement `GetTraces` from the recorded spans (filters per WIRE.md: bucket, replica, limit); the Traces tab decodes real `RequestTrace`/`TraceSpan` (proto fields 1–21 incl. the resource state) for live and replay; `traces.ts`'s generator is deleted; if U24's sampler is off by default, the walkthrough scripts and the dashboard's StartRun send `record_traces=true`. Harness: on a live run the Traces tab lists ≥1 sampled request with spans whose replica ids exist in the fleet.

### U105 rewind re-simulates from the idle guard's checkpoint (queued after the lanes' current units, not this wave)
`Rewind` answers 501 and is hidden in the UI; no unit existed. Re-simulate from the checkpoint the idle guard already writes.

### U106 physics knobs editable on a live run, applied by a restart (default model, after U100)
Issao: *"where do i tune step token budget?"* Main: the Cluster tab's physics knobs (step token budget, prefill rate, KV per replica, max queue, accelerator, replicas) are greyed "server" and inert because UpdateWorkload/UpdatePolicies take only workload/policy keys and there is no restart path. On live they become editable; a change shows "restart the run to apply: <keys>" with a Restart button that sends StopRun + StartRun with the edited config at the same seed and reopens the subscription (`restart(next)` on `RunHandle` is the seam); workload/policy knobs keep applying live; on replay everything stays locked as "recording". Each physics knob gets a one-line explanation in docs/calibration.md's words. Harness: change the step token budget on a live run, assert the banner, press Restart, assert a new run id and the new value in the run's scenario.

### U107 app-shell layout: the page never scrolls, panels do (default model, with U99)
**Spawned 21:54** on `claude/tl-app-shell`, port 8206. Issao, verbatim: *"the scrolling of control seems wrong, i think it should scroll inside the panel, not the whole page, otherwise the 'subscriptions open...' line at the bottom stays over it. In general the full screen shouldn't scroll, just individual panels."* Dashboard, A/B and showcase become an app shell: header, playback bar and status bar fixed rows; a 100dvh body grid where the control and observation panels scroll independently (overflow auto on the panel body, min-height 0 on every ancestor); html/body overflow hidden scoped to those routes; Home and reports keep page scrolling. Harness: scrollingElement.scrollHeight <= innerHeight, the control panel scrolls when wheeled, the status bar never overlaps the last control; screenshots at 1400×900 and 1280×720.

### U108 lockstep A/B through StepForward (queued, default model)
v2 of U96: the two live runs advance in lockstep, each step a `StepForward` on both, so the shared cursor is equal by construction rather than the earlier of two edges. Needs the paced/stepped switch on `ServerRunEngine`; after U96.

## Waiting on Issao

### U42 SLO class targets (answered)
Per-class targets for U25: interactive TTFT 2 s / ITL 80 ms, agent 5 s / 150 ms, batch 60 s / no ITL
target. Issao, in this file at 16:45: *"That looks good. ideally we would have an average throughput for
batch averaged at a longer time window, but don't worry about it for now, record it for future work."*
Accepted; the future work is U44.

### U44 batch-class throughput over a longer window (future work, per Issao)
The batch SLO class has no inter-token target; its service quality is throughput averaged over a window
much longer than a sample interval (minutes, not 250 ms). Add a per-class windowed throughput metric with a
configurable window to the scorecard and the export once U25 exists. Recorded at Issao's request; not
scheduled.

### U43 rule-set version sign-off
U20 bumps the arena rule set to v2 (share-of-offered-work objective, cap 0.95). Every score records its
version. Default: v2 stands.
