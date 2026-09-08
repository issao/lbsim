import type { ReactNode } from 'react';
import { fieldLabel } from '../lib/wired';

/**
 * U95b: the value of a field the engine does not produce yet. Issao's rule: no panel may show an
 * invented number, so the cell says so instead of carrying a placeholder that reads like a
 * measurement. `what` is the field's
 * identifier (see `FIELD_LABEL`) or free text, and goes into the hover so the reader knows which
 * value is missing, not only that one is.
 */
export function Unwired({ what }: { what?: string }) {
  const title = what ? `${fieldLabel(what)}: not simulated yet` : 'not simulated yet';
  return (
    <span className="unwired" title={title} style={{ color: 'var(--ink-3)' }}>
      —
    </span>
  );
}

export function Panel({
  title,
  sub,
  right,
  children,
  bodyClass = '',
  highlight = false,
  id,
}: {
  title: string;
  sub?: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  bodyClass?: string;
  highlight?: boolean;
  id?: string;
}) {
  return (
    <section className={`panel${highlight ? ' highlight' : ''}`} id={id} data-panel={id}>
      <header className="panel-head">
        <span className="panel-title">{title}</span>
        {sub ? (
          <span className="panel-sub" title={typeof sub === 'string' ? sub : undefined}>
            {sub}
          </span>
        ) : null}
        {right ? <span className="panel-head-right">{right}</span> : null}
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
  id,
  dataTile,
}: {
  label: string;
  /** A formatted number, or `<Unwired/>` for a field the engine does not produce yet. */
  value: ReactNode;
  unit?: string;
  note?: ReactNode;
  status?: Status;
  statusText?: string;
  id?: string;
  dataTile?: string;
}) {
  // U99: the flag and note rows are always in the DOM, at a fixed height, so a tile never grows or
  // shrinks because the status text or note happened to show up this frame -- `visibility: hidden`
  // reserves the line instead of unmounting it.
  const hasFlag = Boolean(status && statusText);
  const hasNote = note !== undefined && note !== null && note !== '';
  const noteTitle = typeof note === 'string' ? note : undefined;
  return (
    <div className={`tile${status ? ` status-${status}` : ''}`} id={id} data-tile={dataTile}>
      <div className="tile-label" title={label}>
        {label}
      </div>
      <div className="tile-value num">
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
      <div className={`tile-flag${hasFlag ? '' : ' empty'}`} title={hasFlag ? statusText : undefined}>
        <i className={`dot ${status ?? ''}`} />
        <span>{statusText}</span>
      </div>
      <div className={`tile-note${hasNote ? '' : ' empty'}`} title={noteTitle}>
        {note}
      </div>
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
  readonly,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
  format?: (v: number) => string;
  note?: ReactNode;
  readonly?: boolean;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      <span className="field-value">{format ? format(value) : value}</span>
      {readonly ? (
        <span className="field-value" />
      ) : (
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={value}
          onChange={(e) => onChange(Number(e.target.value))}
        />
      )}
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
