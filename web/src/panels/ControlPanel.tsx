import { useState, type ReactNode } from 'react';
import type { RunHandle } from '../lib/useRun';
import type { RoutingConfig, ScenarioConfig } from '../lib/config';
import { cloneConfig, PRESETS, ROUTING_LABEL, ROUTING_NOTE, VIEW_ONLY_EXPLANATION, diffConfig } from '../lib/config';
import type { RoutingKind } from '../lib/types';
import { estimatedFleetRps, ratedFleetRps } from '../lib/engine';
import { Check, Panel, Select, Slider, Tabs, type TabDef } from '../components/ui';
import { fmtNum, fmtTime, fmtTokens } from '../lib/format';

export type ControlTab = 'scenarios' | 'load' | 'policies' | 'cluster' | 'run';

const TABS: TabDef<ControlTab>[] = [
  { id: 'scenarios', label: 'Scenarios' },
  { id: 'load', label: 'Load' },
  { id: 'policies', label: 'Policies' },
  { id: 'cluster', label: 'Cluster' },
  { id: 'run', label: 'Run' },
];

/** A tab whose knobs a recording cannot act on: greyed, inert, and saying why, in the shape of
 *  the tab bodies' own `Dropped` wrapper (see below in this file). */
function ReplayLocked({ reason, children }: { reason: string | null; children: ReactNode }) {
  return (
    <fieldset className="dropped" disabled title={reason ?? undefined}>
      {children}
      <span className="dropped-why">recording</span>
    </fieldset>
  );
}

export function ControlPanel({
  run,
  tab,
  onTab,
  highlight,
}: {
  run: RunHandle;
  tab: ControlTab;
  onTab: (t: ControlTab) => void;
  highlight?: string | null;
}) {
  const c = run.config;
  const set = (mut: (draft: ScenarioConfig) => void) => {
    const next = cloneConfig(c);
    mut(next);
    run.update(next);
  };
  // Config paths a live server does not take. Naming them in a banner left the knobs looking
  // live; they are disabled where they stand instead.
  const dropped: string[] = 'dropped' in run ? (run as { dropped: string[] }).dropped : [];
  // A recording cannot change: load/policy/cluster knobs go inert with a one-word reason instead
  // of looking live while doing nothing.
  const isReplay = run.source?.kind === 'replay';
  const replayReason = run.source?.disabledReason ?? null;
  const lockOnReplay = (body: ReactNode) => (isReplay ? <ReplayLocked reason={replayReason}>{body}</ReplayLocked> : body);

  return (
    <Panel
      title="Control"
      sub={c.name}
      bodyClass="tight"
      highlight={highlight === 'control'}
      id="control"
      data={{ kind: 'real', mockFields: [] }}
    >
      <Tabs tabs={TABS} value={tab} onChange={onTab} scope="control" />
      <div style={{ padding: 9, overflow: 'auto' }}>
        {tab === 'scenarios' ? <ScenariosTab run={run} /> : null}
        {tab === 'load' ? lockOnReplay(<LoadTab c={c} set={set} dropped={dropped} />) : null}
        {tab === 'policies' ? lockOnReplay(<PoliciesTab c={c} set={set} dropped={dropped} />) : null}
        {tab === 'cluster' ? lockOnReplay(<ClusterTab c={c} set={set} dropped={dropped} />) : null}
        {tab === 'run' ? <RunTab run={run} set={set} /> : null}
      </div>
    </Panel>
  );
}

// ---------------------------------------------------------------------------

function ScenariosTab({ run }: { run: RunHandle }) {
  return (
    <>
      <p className="note" style={{ marginTop: 0 }}>
        One click each. The real call is <code>StartRun(Scenario)</code>; the file named beside each preset is
        what would be sent.
      </p>
      {PRESETS.map((p) => {
        const target = p.apply(run.config);
        const differing = diffConfig(run.config, target).paths.filter((x) => x !== 'name');
        const active = differing.length === 0;
        return (
          <div key={p.id} style={{ borderTop: '1px solid var(--line)', padding: '7px 0' }}>
            <div style={{ display: 'flex', gap: 6, alignItems: 'baseline' }}>
              <b style={{ fontSize: 11.5, fontWeight: 600 }}>{p.title}</b>
              {active ? <span className="note" style={{ color: 'var(--accent-ink)' }}>loaded</span> : null}
            </div>
            <p className="note" style={{ margin: '1px 0 4px' }}>{p.summary}</p>
            <div className="btn-row">
              <button className="btn" disabled={active} onClick={() => run.restart(p.apply(run.config))}>
                load and restart
              </button>
              <code className="note">{p.file}</code>
            </div>
          </div>
        );
      })}
    </>
  );
}

// ---------------------------------------------------------------------------

type TabProps = { c: ScenarioConfig; set: (m: (d: ScenarioConfig) => void) => void; dropped?: string[] };

/** A field the current server does not take: greyed and inert, with the one word that says why. */
function Dropped({ path, dropped, children }: { path: string; dropped?: string[]; children: ReactNode }) {
  if (!dropped?.includes(path)) return <>{children}</>;
  return (
    <fieldset className="dropped" disabled title={`the server does not take ${path}`}>
      {children}
      <span className="dropped-why">server</span>
    </fieldset>
  );
}

function LoadTab({ c, set, dropped }: TabProps) {
  const rated = ratedFleetRps(c);
  const est = estimatedFleetRps(c);
  const rho = c.workload.arrivalRps / Math.max(est, 1e-9);
  return (
    <>
      <p className="section-label">Arrivals</p>
      <Slider
        label="arrival rate"
        value={c.workload.arrivalRps}
        min={10}
        max={400}
        step={5}
        format={(v) => `${v} rps`}
        onChange={(v) => set((d) => { d.workload.arrivalRps = v; })}
        note={
          <>
            capacity {fmtNum(est, 0)} rps once parked sessions have their share of the cache, against a{' '}
            {fmtNum(rated, 0)} rps nameplate &middot; offered/capacity{' '}
            <b style={{ color: rho > 1 ? 'var(--critical)' : rho > 0.85 ? 'var(--serious)' : 'var(--ink-2)' }}>
              {rho.toFixed(2)}
            </b>
          </>
        }
      />
      <Dropped path="workload.perturbation" dropped={dropped}>
      <Select
        label="perturbation"
        value={c.workload.perturbation}
        options={[
          { value: 'none', label: 'none' },
          { value: 'step', label: 'step' },
          { value: 'sinusoid', label: 'sinusoid' },
        ]}
        onChange={(v) => set((d) => { d.workload.perturbation = v; })}
        note="an independent input layered on the baseline, for control analysis"
      />
      {c.workload.perturbation !== 'none' ? (
        <>
          <Slider
            label="amplitude"
            value={c.workload.perturbAmplitude}
            min={0.05}
            max={1.5}
            step={0.05}
            format={(v) => `${(v * 100).toFixed(0)}%`}
            onChange={(v) => set((d) => { d.workload.perturbAmplitude = v; })}
          />
          {c.workload.perturbation === 'sinusoid' ? (
            <Slider
              label="frequency"
              value={c.workload.perturbFrequencyHz}
              min={0.005}
              max={0.5}
              step={0.005}
              format={(v) => `${v.toFixed(3)} Hz`}
              onChange={(v) => set((d) => { d.workload.perturbFrequencyHz = v; })}
            />
          ) : null}
        </>
      ) : null}
      </Dropped>

      <div className="sep" />
      <p className="section-label">Lengths</p>
      <Slider
        label="prompt mean"
        value={c.workload.promptMean}
        min={100}
        max={8000}
        step={100}
        format={(v) => `${fmtTokens(v)} tok`}
        onChange={(v) => set((d) => { d.workload.promptMean = v; })}
      />
      <Slider
        label="prompt spread"
        value={c.workload.promptCv}
        min={0.1}
        max={3}
        step={0.1}
        format={(v) => `cv ${v.toFixed(1)}`}
        onChange={(v) => set((d) => { d.workload.promptCv = v; })}
        note="coefficient of variation; the tail is what creates head-of-line blocking"
      />
      <Slider
        label="output mean"
        value={c.workload.outputMean}
        min={25}
        max={2000}
        step={25}
        format={(v) => `${fmtTokens(v)} tok`}
        onChange={(v) => set((d) => { d.workload.outputMean = v; })}
      />
      <Slider
        label="output spread"
        value={c.workload.outputCv}
        min={0.1}
        max={3}
        step={0.1}
        format={(v) => `cv ${v.toFixed(1)}`}
        onChange={(v) => set((d) => { d.workload.outputCv = v; })}
      />

      <div className="sep" />
      <p className="section-label">The long tail</p>
      <Slider
        label="long-request probability"
        value={c.workload.longProbability}
        min={0}
        max={0.4}
        step={0.01}
        format={(v) => `${(v * 100).toFixed(0)}%`}
        onChange={(v) => set((d) => { d.workload.longProbability = v; })}
        note="turn this to zero and round robin stops producing a hotspot: the imbalance is size heterogeneity, not the policy alone"
      />
      <Slider
        label="long prompt mean"
        value={c.workload.longPromptMean}
        min={4000}
        max={64000}
        step={1000}
        format={(v) => `${fmtTokens(v)} tok`}
        onChange={(v) => set((d) => { d.workload.longPromptMean = v; })}
      />
      <Slider
        label="long output mean"
        value={c.workload.longOutputMean}
        min={100}
        max={2000}
        step={50}
        format={(v) => `${fmtTokens(v)} tok`}
        onChange={(v) => set((d) => { d.workload.longOutputMean = v; })}
      />
      <p className="note inset">
        <code>UpdateWorkload</code> replaces the whole <code>LoadShape</code> rather than patching it: arrival rate,
        lengths and prefix topology interact, so half a change would produce a load nobody asked for.
      </p>
    </>
  );
}

// ---------------------------------------------------------------------------

/**
 * Controls generated from the routing policy variant, the way the real panel is generated from
 * PolicySpec, so the panel cannot offer a parameter the engine would not accept.
 */
function PoliciesTab({ c, set, dropped }: TabProps) {
  const kinds = Object.keys(ROUTING_LABEL) as RoutingKind[];
  return (
    <>
      <p className="section-label">Routing</p>
      <Select
        label="policy"
        value={c.routing.kind}
        options={kinds.map((k) => ({ value: k, label: ROUTING_LABEL[k] }))}
        onChange={(v) => set((d) => { d.routing.kind = v; })}
      />
      <p className="note inset" style={{ margin: '-4px 0 10px' }}>{ROUTING_NOTE[c.routing.kind]}</p>
      <RoutingParams routing={c.routing} set={set} dropped={dropped} />

      <div className="sep" />
      <p className="section-label">What the policy sees</p>
      <Slider
        label="telemetry interval"
        value={c.telemetryIntervalMs}
        min={100}
        max={4000}
        step={100}
        format={(v) => `${v} ms`}
        onChange={(v) => set((d) => { d.telemetryIntervalMs = v; })}
      />
      <Slider
        label="telemetry delay"
        value={c.telemetryDelayMs}
        min={0}
        max={2000}
        step={50}
        format={(v) => `${v} ms`}
        onChange={(v) => set((d) => { d.telemetryDelayMs = v; })}
        note="the observation is never the live state. Raise it under least-KV-tokens and the fleet oscillates; raise it under power-of-two-choices and almost nothing happens."
      />

      <div className="sep" />
      <p className="section-label">
        SLO targets{' '}
        <span className="mock-tag" style={{ textTransform: 'none' }}>
          view only
        </span>
      </p>
      <Slider
        label="TTFT SLO"
        value={c.slo.ttftMs}
        min={200}
        max={10000}
        step={100}
        format={(v) => `${v} ms`}
        onChange={(v) => set((d) => { d.slo.ttftMs = v; })}
      />
      <Slider
        label="ITL SLO"
        value={c.slo.itlMs}
        min={10}
        max={400}
        step={5}
        format={(v) => `${v} ms`}
        onChange={(v) => set((d) => { d.slo.itlMs = v; })}
      />
      <Slider
        label="end-to-end SLO"
        value={c.slo.e2eS}
        min={5}
        max={300}
        step={5}
        format={(v) => `${v} s`}
        onChange={(v) => set((d) => { d.slo.e2eS = v; })}
      />
      <p className="note inset">
        These three move goodput and attainment without re-simulating anything: {VIEW_ONLY_EXPLANATION['slo.ttftMs']}.
        The banner above says <code>required_resimulation = false</code> when you move them, and true when you move
        anything in Load, Policies or Cluster.
      </p>
    </>
  );
}

function RoutingParams({
  routing,
  set,
  dropped,
}: {
  routing: RoutingConfig;
  set: (m: (d: ScenarioConfig) => void) => void;
  dropped?: string[];
}) {
  switch (routing.kind) {
    case 'power_of_two_choices':
      return (
        <>
          <Slider
            label="choices (d)"
            value={routing.choices}
            min={2}
            max={8}
            step={1}
            format={(v) => `${v}`}
            onChange={(v) => set((d) => { d.routing.choices = v; })}
            note="2 is where almost all the benefit lives"
          />
          <Check
            label="probe live instead of reading the snapshot"
            value={routing.probeLive}
            onChange={(v) => set((d) => { d.routing.probeLive = v; })}
            note="pays a modelled RPC, so the price of freshness is visible rather than free"
          />
        </>
      );
    case 'prefix_affinity':
      return (
        <>
          <Dropped path="routing.maxLoadRatio" dropped={dropped}>
            <Slider
              label="max load ratio"
              value={routing.maxLoadRatio}
              min={1}
              max={3}
              step={0.05}
              format={(v) => v.toFixed(2)}
              onChange={(v) => set((d) => { d.routing.maxLoadRatio = v; })}
              note="1.00 disables affinity entirely; large values ignore load and produce hotspots"
            />
          </Dropped>
          <Dropped path="routing.fallbackChoices" dropped={dropped}>
            <Slider
              label="fallback choices"
              value={routing.fallbackChoices}
              min={1}
              max={8}
              step={1}
              format={(v) => `${v}`}
              onChange={(v) => set((d) => { d.routing.fallbackChoices = v; })}
            />
          </Dropped>
        </>
      );
    default:
      return <p className="note inset">This policy takes no parameters. <code>PolicySpec</code> has no fields for it, so the panel shows none.</p>;
  }
}

// ---------------------------------------------------------------------------

function ClusterTab({ c, set, dropped }: TabProps) {
  return (
    <>
      <p className="section-label">Fleet shape</p>
      <p className="note" style={{ marginTop: -2 }}>
        1 cluster, 1 pool. Multi-cluster and multi-pool are phase 2 in <code>docs/ARCHITECTURE.md</code> section 12, so
        this panel does not pretend to offer them yet.
      </p>
      <Slider
        label="replicas per pool"
        value={c.fleet.replicas}
        min={4}
        max={128}
        step={4}
        format={(v) => `${v}`}
        onChange={(v) => set((d) => { d.fleet.replicas = v; })}
        note="changing fleet size restarts the run: a snapshot of a differently shaped fleet cannot be restored"
      />
      <Slider
        label="max batch"
        value={c.fleet.maxBatch}
        min={1}
        max={128}
        step={1}
        format={(v) => `${v} seq`}
        onChange={(v) => set((d) => { d.fleet.maxBatch = v; })}
        note="raising it buys throughput and costs inter-token latency, because step time grows with the batch"
      />
      <Slider
        label="KV capacity per replica"
        value={c.fleet.kvTokensPerReplica}
        min={10000}
        max={400000}
        step={5000}
        format={(v) => `${fmtTokens(v)} tok`}
        onChange={(v) => set((d) => { d.fleet.kvTokensPerReplica = v; })}
        note="lower it until preemption starts and throughput falls as load rises"
      />
      <Slider
        label="max queue"
        value={c.fleet.maxQueue}
        min={10}
        max={1000}
        step={10}
        format={(v) => `${v} req`}
        onChange={(v) => set((d) => { d.fleet.maxQueue = v; })}
      />

      <div className="sep" />
      <p className="section-label">Accelerator model</p>
      <Dropped path="fleet.accelerator" dropped={dropped}>
      <Select
        label="accelerator"
        value={c.fleet.accelerator}
        options={[
          { value: '8xH100-80GB', label: '8x H100 80GB' },
          { value: '8xA100-80GB', label: '8x A100 80GB' },
          { value: '4xH100-80GB', label: '4x H100 80GB' },
        ]}
        onChange={(v) =>
          set((d) => {
            d.fleet.accelerator = v;
            // The three presets differ in the epoch cost model, not just in a label.
            if (v === '8xA100-80GB') { d.fleet.stepBaseMs = 4.6; d.fleet.stepPerSeqMs = 0.42; d.fleet.prefillTokensPerS = 13000; }
            else if (v === '4xH100-80GB') { d.fleet.stepBaseMs = 4.1; d.fleet.stepPerSeqMs = 0.44; d.fleet.prefillTokensPerS = 13500; }
            else { d.fleet.stepBaseMs = 2.75; d.fleet.stepPerSeqMs = 0.25; d.fleet.prefillTokensPerS = 25000; }
          })
        }
      />
      </Dropped>
      <Slider
        label="step base"
        value={c.fleet.stepBaseMs}
        min={0.5}
        max={12}
        step={0.05}
        format={(v) => `${v.toFixed(2)} ms`}
        onChange={(v) => set((d) => { d.fleet.stepBaseMs = v; })}
      />
      <Slider
        label="step per sequence"
        value={c.fleet.stepPerSeqMs}
        min={0.02}
        max={1.2}
        step={0.01}
        format={(v) => `${v.toFixed(2)} ms`}
        onChange={(v) => set((d) => { d.fleet.stepPerSeqMs = v; })}
      />
      <Slider
        label="prefill rate"
        value={c.fleet.prefillTokensPerS}
        min={4000}
        max={60000}
        step={1000}
        format={(v) => `${fmtTokens(v)} tok/s`}
        onChange={(v) => set((d) => { d.fleet.prefillTokensPerS = v; })}
      />
      <p className="note inset">
        <code>step_base_ms</code> is calibrated in <code>bench/validate_epochs.py</code> against a published batch-1
        measurement. The numbers drawn from them here are not.
      </p>
    </>
  );
}

// ---------------------------------------------------------------------------

function RunTab({ run, set }: { run: RunHandle; set: (m: (d: ScenarioConfig) => void) => void }) {
  const [seedText, setSeedText] = useState(String(run.config.seed));
  const c = run.config;
  return (
    <>
      <p className="section-label">Now</p>
      <dl className="kv">
        <dt>run id</dt>
        <dd>{run.source?.runId ?? `mock-${c.name}-${c.seed}`}</dd>
        <dt>sim time</dt>
        <dd>{fmtTime(run.cursorS)}</dd>
        <dt>recorded to</dt>
        <dd>{run.recordedToS.toFixed(1)} s</dd>
        <dt>duration</dt>
        <dd>{c.durationS} s</dd>
        <dt>state</dt>
        <dd>{run.resimulating ? 'resimulating' : run.paused ? 'paused' : `running ${run.speed}x`}</dd>
      </dl>

      <Slider
        label="sample rate"
        value={c.samplesPerSimSecond}
        min={0.5}
        max={2}
        step={0.5}
        format={(v) => `${v}/sim s`}
        onChange={(v) => set((d) => { d.samplesPerSimSecond = v; })}
        note="points per simulated second, so chart density does not change when the speed does. View only."
      />

      <div className="sep" />
      <p className="section-label">Reproducibility</p>
      <div className="field">
        <span className="field-label">seed</span>
        <span />
        <input
          type="text"
          value={seedText}
          onChange={(e) => setSeedText(e.target.value)}
          style={{ gridColumn: '1 / -1' }}
        />
        <span className="field-note">
          named independent streams mean the workload does not shift when the policy changes, so a visible difference
          is a difference in policy rather than in luck
        </span>
      </div>
      <div className="btn-row">
        <button
          className="btn"
          onClick={() => {
            const n = Number(seedText);
            if (Number.isFinite(n)) {
              const next = { ...c, seed: Math.floor(n) };
              run.restart(next);
            }
          }}
        >
          restart with seed
        </button>
        <button className="btn" onClick={() => run.restart(c)}>
          restart
        </button>
      </div>
    </>
  );
}
