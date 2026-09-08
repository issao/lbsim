import { useSyncExternalStore, type ReactNode } from 'react';
import { activeMode, DATA_SOURCE_GLOSS, subscribeActiveMode } from '../lib/mode';
import { panelTagWord } from '../lib/wired';

/**
 * Every panel carries this. Per docs/ui-spec.md section 5: a dashboard that looks real while
 * showing invented numbers is how someone ends up trusting a chart that was never connected.
 *
 * U70: the same three words everywhere -- mock, replay, live -- each glossed in the tooltip on
 * whichever word this instance shows. `what` also takes free text (a caller's own longer label);
 * only the three canonical words carry a gloss, so free text keeps its old generic tooltip.
 */
export function MockTag({ what = 'mock', fields }: { what?: string; fields?: string[] }) {
  const gloss = (DATA_SOURCE_GLOSS as Record<string, string | undefined>)[what];
  const title =
    fields && fields.length > 0
      ? `mock: ${fields.join(', ')}`
      : gloss
        ? `${what} — ${gloss}`
        : 'Generated in the browser. Not connected to sim-ingress.';
  return (
    <span className="mock-tag" title={title}>
      {what}
    </span>
  );
}

/** What a panel may claim about the frame it drew, from `realness()` in lib/wired.ts. */
export interface PanelData {
  kind: 'mock' | 'partial' | 'real';
  mockFields: string[];
}

export function Panel({
  title,
  sub,
  right,
  children,
  bodyClass = '',
  highlight = false,
  id,
  data,
}: {
  title: string;
  sub?: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  bodyClass?: string;
  highlight?: boolean;
  id?: string;
  /** Whether the frame behind this panel is mock, partially wired, or real. Absent means unknown,
   *  which is treated as mock: no panel renders a number without one of the three words in its tag. */
  data?: PanelData;
}) {
  // U70: a panel that reads any unwired field -- or draws from a mock frame at all -- is tagged
  // `mock`, full stop. Only a panel where every field it reads is wired earns the active source's
  // own word (`replay` or `live`), because that is the only case where "real" and "mock" differ.
  const active = useSyncExternalStore(subscribeActiveMode, activeMode);
  // none/connecting/refused are badge states, not data sources: no engine numbers are on screen yet, so the tag word is mock.
  const tagWord = panelTagWord(data, active.mode === 'server' || active.mode === 'replay' ? active.mode : 'mock');
  return (
    <section className={`panel${highlight ? ' highlight' : ''}`} id={id} data-panel={id}>
      <header className="panel-head">
        <span className="panel-title">{title}</span>
        {sub ? <span className="panel-sub">{sub}</span> : null}
        <span className="panel-head-right">
          {right}
          <MockTag what={tagWord} fields={data?.kind === 'partial' ? data.mockFields : undefined} />
        </span>
      </header>
      <div className={`panel-body ${bodyClass}`}>{children}</div>
    </section>
  );
}

export interface TabDef<T extends string> {
  id: T;
  label: string;
  count?: number;
}

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
  scope,
}: {
  tabs: TabDef<T>[];
  value: T;
  onChange: (v: T) => void;
  scope: string;
}) {
  return (
    <div className="tabs" role="tablist">
      {tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={t.id === value}
          onClick={() => onChange(t.id)}
          data-tab={`${scope}:${t.id}`}
        >
          {t.label}
          {t.count !== undefined ? <span className="tab-count">{t.count}</span> : null}
        </button>
      ))}
    </div>
  );
}

export type Status = 'good' | 'warning' | 'serious' | 'critical' | 'info';

/**
 * A status colour never carries meaning alone: each tile pairs it with a dot and a word.
 */
export function Tile({
  label,
  value,
  unit,
  note,
  status,
  statusText,
}: {
  label: string;
  value: string;
  unit?: string;
  note?: ReactNode;
  status?: Status;
  statusText?: string;
}) {
  return (
    <div className={`tile${status ? ` status-${status}` : ''}`}>
      <div className="tile-label">{label}</div>
      <div className="tile-value num">
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
      {status && statusText ? (
        <div className="tile-flag">
          <i className={`dot ${status}`} />
          <span>{statusText}</span>
        </div>
      ) : null}
      {note ? <div className="tile-note">{note}</div> : null}
    </div>
  );
}

export function Slider({
  label,
  value,
  min,
  max,
  step,
  onChange,
  format,
  note,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  format?: (v: number) => string;
  note?: ReactNode;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <span className="field-value">{format ? format(value) : value}</span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      {note ? <span className="field-note">{note}</span> : null}
    </label>
  );
}

export function Select<T extends string>({
  label,
  value,
  options,
  onChange,
  note,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  note?: ReactNode;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <span />
      <select value={value} onChange={(e) => onChange(e.target.value as T)} style={{ gridColumn: '1 / -1' }}>
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      {note ? <span className="field-note">{note}</span> : null}
    </label>
  );
}

export function Check({
  label,
  value,
  onChange,
  note,
}: {
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
  note?: ReactNode;
}) {
  return (
    <>
      <label className="check">
        <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} />
        <span>{label}</span>
      </label>
      {note ? <div className="field-note" style={{ marginTop: -6, marginBottom: 9 }}>{note}</div> : null}
    </>
  );
}

export function NumberField({
  label,
  value,
  onChange,
  note,
  step = 1,
}: {
  label: string;
  value: number;
  onChange: (v: number) => void;
  note?: ReactNode;
  step?: number;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <span />
      <input
        type="number"
        value={value}
        step={step}
        onChange={(e) => onChange(Number(e.target.value))}
        style={{ gridColumn: '1 / -1' }}
      />
      {note ? <span className="field-note">{note}</span> : null}
    </label>
  );
}
