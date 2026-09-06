import { useMemo, useState } from 'react';
import type { Frame } from '../../lib/engine';
import type { ScenarioConfig } from '../../lib/config';
import { getTraces } from '../../lib/traces';
import { OUTCOMES, TRACE_BUCKETS, type Outcome, type TraceBucket } from '../../lib/types';
import { Panel } from '../../components/ui';
import { Waterfall } from '../../components/charts/Waterfall';
import { fmtMs, fmtTokens } from '../../lib/format';

function outcomeStatus(o: Outcome): 'good' | 'warning' | 'serious' | 'critical' {
  if (o === 'OK') return 'good';
  if (o === 'OK_SLO_VIOLATED') return 'serious';
  if (o === 'REJECTED') return 'warning';
  return 'critical';
}

/**
 * Traces are a query against what the recorder already sampled, so this tab does not pause the run.
 * Pausing is offered as a convenience, because a reader usually wants the surrounding charts to stop
 * moving while they study one request -- but a view that forces a pause cannot be used to watch a
 * dynamic unfold, which is much of the point.
 */
export function Traces({
  frames,
  config,
  paused,
  onPause,
  highlight,
}: {
  frames: Frame[];
  config: ScenarioConfig;
  paused: boolean;
  onPause: (p: boolean) => void;
  highlight?: string | null;
}) {
  const [bucket, setBucket] = useState<TraceBucket>('p99');
  const [outcome, setOutcome] = useState<Outcome | 'any'>('any');
  const [selected, setSelected] = useState(0);

  const traces = useMemo(
    () => getTraces(config, frames, { bucket, outcome, limit: 8 }),
    // The trace set is stable while the frame window is: sampling is deterministic.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [config, bucket, outcome, frames.length, frames[0]?.tick]
  );
  const trace = traces[Math.min(selected, Math.max(traces.length - 1, 0))];

  return (
    <div className="grid" style={{ gridTemplateColumns: 'minmax(280px, 380px) minmax(0, 1fr)' }}>
      <Panel
        title="Sampled traces"
        sub="by latency bucket"
        bodyClass="tight"
        highlight={highlight === 'trace-list'}
        id="trace-list"
        right={
          <button className="btn" onClick={() => onPause(!paused)} title="A convenience, not a requirement of GetTraces">
            {paused ? 'resume charts' : 'pause charts'}
          </button>
        }
      >
        <div style={{ display: 'flex', gap: 6, padding: '6px 7px', alignItems: 'center', flexWrap: 'wrap' }}>
          <span className="seg">
            {TRACE_BUCKETS.map((b) => (
              <button key={b} aria-pressed={bucket === b} onClick={() => { setBucket(b); setSelected(0); }}>
                {b}
              </button>
            ))}
          </span>
          <select
            value={outcome}
            onChange={(e) => { setOutcome(e.target.value as Outcome | 'any'); setSelected(0); }}
            style={{ width: 'auto', flex: '1 1 90px' }}
            aria-label="outcome filter"
          >
            <option value="any">any outcome</option>
            {OUTCOMES.map((o) => (
              <option key={o} value={o}>{o.toLowerCase().replace(/_/g, ' ')}</option>
            ))}
          </select>
        </div>
        <table className="data">
          <thead>
            <tr>
              <th className="nosort">request</th>
              <th className="nosort">rep</th>
              <th className="nosort">prompt</th>
              <th className="nosort">ttft</th>
              <th className="nosort">e2e</th>
            </tr>
          </thead>
          <tbody>
            {traces.map((t, i) => (
              <tr key={t.requestId} className={t === trace ? 'sel' : ''} onClick={() => setSelected(i)} style={{ cursor: 'pointer' }}>
                <td style={{ textAlign: 'left' }}>
                  <i className={`dot ${outcomeStatus(t.outcome)}`} style={{ marginRight: 5 }} />
                  <span className="num">{t.requestId}</span>
                </td>
                <td className="n">{t.replicaId}</td>
                <td className="n">{fmtTokens(t.promptTokens)}</td>
                <td className={`n${t.ttftMs > config.slo.ttftMs ? ' bad' : ''}`}>{fmtMs(t.ttftMs)}</td>
                <td className="n">{fmtMs(t.e2eMs)}</td>
              </tr>
            ))}
            {traces.length === 0 ? (
              <tr>
                <td colSpan={5} style={{ textAlign: 'left', color: 'var(--ink-3)' }}>
                  no sampled trace in this bucket matches that outcome
                </td>
              </tr>
            ) : null}
          </tbody>
          <caption>
            Uniform sampling of hundreds of millions of requests contains almost no examples above the 99.9th
            percentile, and those are the only ones worth reading. Hence buckets.
          </caption>
        </table>
      </Panel>

      {trace ? (
        <Panel
          title={`Request ${trace.requestId}`}
          sub={`${trace.bucket} · ${trace.outcome.toLowerCase().replace(/_/g, ' ')} · arrived at ${trace.arrivalSimS.toFixed(1)} s`}
          highlight={highlight === 'waterfall'}
          id="waterfall"
        >
          <div className="grid c4" style={{ gap: 6, marginBottom: 8 }}>
            <div className="tile">
              <div className="tile-label">routed to</div>
              <div className="tile-value num">{trace.replicaId}</div>
              <div className="tile-note">
                considered {trace.consideredIds.length === 1 ? 'no alternatives' : `${trace.consideredIds.length}: ${trace.consideredIds.slice(0, 6).join(', ')}${trace.consideredIds.length > 6 ? '…' : ''}`}
              </div>
            </div>
            <div className="tile">
              <div className="tile-label">cache belief</div>
              <div className="tile-value num">{fmtTokens(trace.predictedCacheHitTokens)}</div>
              <div className="tile-note">
                predicted; actual {fmtTokens(trace.actualCacheHitTokens)} (
                {trace.predictedCacheHitTokens > trace.actualCacheHitTokens ? 'over' : 'under'}estimated)
              </div>
            </div>
            <div className="tile">
              <div className="tile-label">tokens</div>
              <div className="tile-value num">
                {fmtTokens(trace.promptTokens)}<small> / {fmtTokens(trace.outputTokens)}</small>
              </div>
              <div className="tile-note">prompt / output</div>
            </div>
            <div className="tile">
              <div className="tile-label">tenant</div>
              <div className="tile-value" style={{ fontSize: 13 }}>{trace.tenant}</div>
              <div className="tile-note">{trace.sloClass.toLowerCase()} class</div>
            </div>
          </div>
          <Waterfall trace={trace} />
          <p className="note" style={{ margin: '8px 0 0' }}>
            Conditions travel with each span: batch size, key-value utilization, tokens processed. The router's recorded
            candidate set is what makes herding measurable, and the gap between predicted and actual cache hit tokens is
            what makes affinity error measurable.
          </p>
        </Panel>
      ) : (
        <Panel title="Request" sub="nothing selected" id="waterfall">
          <p className="note">Pick a trace on the left.</p>
        </Panel>
      )}
    </div>
  );
}
