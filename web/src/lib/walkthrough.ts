// Walkthrough scripts. Content, not interface: a JSON file in the frontend with a scenario
// reference and a list of steps, loaded at runtime from public/walkthroughs/ so the format is
// genuinely exercised rather than inlined into a component. See public/walkthroughs/schema.md.

import type { ControlTab } from '../panels/ControlPanel';
import type { ObserveTab } from '../panels/ObservationPanel';
import { BASE, cloneConfig, type ScenarioConfig } from './config';

export type PatchValue = number | string | boolean;

export interface WalkthroughStep {
  at_sim_s: number;
  speed?: number;
  set?: Record<string, PatchValue>;
  control_tab?: ControlTab;
  observe_tab?: ObserveTab;
  highlight?: string;
  title: string;
  body: string[];
  look_for?: string;
}

export interface WalkthroughScript {
  id: string;
  dynamic: number;
  title: string;
  summary: string;
  scenario: Record<string, PatchValue>;
  steps: WalkthroughStep[];
  /** The exported `sim-run export --demos` run id this walkthrough plays. */
  run?: string;
  /** A second exported run id, for the A/B view. Only meaningful alongside `run`. */
  compare?: string;
}

export interface ShowcaseCard {
  id: string;
  dynamic: number;
  phase: number;
  title: string;
  summary: string;
  requires: string;
  script?: string;
}

export interface ShowcaseIndex {
  note: string;
  cards: ShowcaseCard[];
}

// Computed lazily, not at module scope: the self-test imports this module under plain node
// (no Vite), where `import.meta.env` does not exist, and it only calls validate()/applyPatch().
function baseUrl(): string {
  const env = (import.meta as unknown as { env?: { BASE_URL?: string } }).env;
  return `${env?.BASE_URL ?? '/'}walkthroughs/`;
}

export async function loadIndex(): Promise<ShowcaseIndex> {
  const r = await fetch(`${baseUrl()}index.json`);
  if (!r.ok) throw new Error(`walkthrough index: ${r.status}`);
  return (await r.json()) as ShowcaseIndex;
}

export async function loadScript(file: string): Promise<WalkthroughScript> {
  const r = await fetch(`${baseUrl()}${file}`);
  if (!r.ok) throw new Error(`walkthrough ${file}: ${r.status}`);
  const s = (await r.json()) as WalkthroughScript;
  validate(s);
  return s;
}

/** Fail loudly on a malformed script rather than half-running it. */
export function validate(s: WalkthroughScript): void {
  if (!Array.isArray(s.steps) || s.steps.length === 0) throw new Error(`${s.id}: no steps`);
  let last = -Infinity;
  for (const [i, st] of s.steps.entries()) {
    if (typeof st.at_sim_s !== 'number') throw new Error(`${s.id} step ${i}: at_sim_s missing`);
    if (st.at_sim_s <= last) throw new Error(`${s.id} step ${i}: at_sim_s must increase`);
    last = st.at_sim_s;
    if (!st.title || !Array.isArray(st.body)) throw new Error(`${s.id} step ${i}: title and body required`);
  }
  if (s.run !== undefined && (typeof s.run !== 'string' || !s.run)) {
    throw new Error(`${s.id}: run must be a non-empty string`);
  }
  if (s.compare !== undefined && (typeof s.compare !== 'string' || !s.compare)) {
    throw new Error(`${s.id}: compare must be a non-empty string`);
  }
}

/** Dotted paths, one level deep, matching the field paths the diff and the labels already use. */
export function applyPatch(base: ScenarioConfig, patch: Record<string, PatchValue>): ScenarioConfig {
  const c = cloneConfig(base) as unknown as Record<string, unknown>;
  for (const [path, value] of Object.entries(patch)) {
    const parts = path.split('.');
    if (parts.length === 1) {
      c[parts[0]] = value;
    } else {
      const head = c[parts[0]] as Record<string, unknown> | undefined;
      if (!head) throw new Error(`unknown scenario path ${path}`);
      head[parts[1]] = value;
    }
  }
  return c as unknown as ScenarioConfig;
}

export function scenarioFor(s: WalkthroughScript): ScenarioConfig {
  return applyPatch(BASE, s.scenario);
}
