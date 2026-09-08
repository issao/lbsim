// The walkthrough state machine, with no framework and no engine in it. A script is a list of
// steps, each a simulated timestamp with conditions to set first; the runner applies the
// conditions, moves the run to the timestamp, pauses, and says which step it is on. It is
// written over the smallest handle that can do that, so the same machine drives a recording and a
// live server run, and the self-test drives it with a fake.
//
// Refusals are state, not exceptions. A recording cannot take an override, and a server may
// answer an update with 501 until the call is implemented; either way the step still plays and
// the narration panel shows why the conditions did not change. Throwing here would stop the
// walkthrough at exactly the step whose story the reader came for.

import type { DataMode } from './mode';
import type { PatchValue, WalkthroughScript, WalkthroughStep } from './walkthrough';

export interface RunnerHandle {
  readonly mode: DataMode;
  cursorS(): number;
  scrubTo(s: number): void;
  setSpeed(x: number): void;
  pause(): void;
  play(): void;
  /** Rejects (or throws) with the reason when the run refuses the change. */
  update(patch: Record<string, PatchValue>): Promise<void> | void;
  /** True once the run has finished producing frames; no `tick()` will move the cursor further. Optional; treated as false when absent. */
  ended?(): boolean;
}

export interface StepState {
  index: number;
  step: WalkthroughStep;
  /** Why the step's `set` was not applied, when it was not. The step plays regardless. */
  reason?: string;
  /** Moving toward `step.at_sim_s`; `tick()` pauses the run when it gets there. */
  advancing: boolean;
  /** The viewer paused the run short of `step.at_sim_s`; `resume()` plays on without re-applying `set`. */
  paused: boolean;
  /** The last step has been reached and the run is paused there. */
  done: boolean;
}

export const REPLAY_SET_REFUSED = 'replay of a recording; overrides need a live run';

/** A server run is real work per second, and a recording plays at the same pace so the two read alike. */
export const DEFAULT_SPEED: Record<DataMode, number> = { server: 1, replay: 1 };

export class WalkthroughRunner {
  private st: StepState;
  private readonly script: WalkthroughScript;
  private readonly handle: RunnerHandle;

  constructor(script: WalkthroughScript, handle: RunnerHandle) {
    this.script = script;
    this.handle = handle;
    this.st = { index: -1, step: script.steps[0], advancing: false, paused: false, done: false };
  }

  /** The same object until something changes, so a host can compare by identity. */
  state(): StepState {
    return this.st;
  }

  /**
   * Move to the next step: apply its `set` first, so the conditions are in force for the whole
   * stretch the step narrates, then advance toward its timestamp. On the last step this is a no-op.
   */
  async next(): Promise<StepState> {
    const index = this.st.index + 1;
    if (index >= this.script.steps.length) return this.st;
    const step = this.script.steps[index];
    const reason = step.set ? await this.apply(step.set) : undefined;
    this.st = { index, step, reason, advancing: true, paused: false, done: false };
    this.advance(step);
    return this.st;
  }

  /**
   * The one action behind every play button while a walkthrough is open (the card's and the
   * playback bar's), so both do the same thing. A settled step moves on to the next; a run the
   * viewer paused mid-segment plays again at that segment's speed, with nothing re-applied;
   * anything else (already advancing, or done) is a no-op.
   */
  async resume(): Promise<StepState> {
    if (!this.st.advancing) return this.st.done ? this.st : this.next();
    if (!this.st.paused) return this.st;
    this.st = { ...this.st, paused: false };
    this.handle.setSpeed(this.speedOf(this.st.step));
    this.handle.play();
    return this.st;
  }

  /** The viewer's pause, from the playback bar: stop the run where it is and say so on the card. */
  pauseHere(): StepState {
    if (!this.st.advancing || this.st.paused) return this.st;
    this.handle.pause();
    this.st = { ...this.st, paused: true };
    return this.st;
  }

  /** The speed a step advances at, for the card to show and for `resume()` to restore. */
  speedOf(step: WalkthroughStep): number {
    return step.speed ?? DEFAULT_SPEED[this.handle.mode];
  }

  /**
   * Called by the host whenever the run may have moved. Pauses at the step's timestamp; returns the
   * unchanged state object otherwise.
   *
   * The cursor can arrive past `at_sim_s` rather than exactly on it (a coarse poll interval, or the
   * viewer scrubbing ahead independently). Replay can put itself back exactly, so it rewinds first;
   * a live run stays where it is and says why, rather than visibly rewinding a live view. And a run
   * that ends before `at_sim_s` would otherwise leave the step "advancing…" forever, so an ended
   * handle settles with a reason instead of waiting for a cursor that will never arrive.
   */
  tick(): StepState {
    if (!this.st.advancing) return this.st;
    const { at_sim_s } = this.st.step;
    const cursor = this.handle.cursorS();
    if (cursor > at_sim_s) {
      if (this.handle.mode === 'replay') {
        this.handle.scrubTo(at_sim_s);
        this.settle();
      } else {
        this.settle('the run is already past this step');
      }
      return this.st;
    }
    if (cursor === at_sim_s) {
      this.settle();
      return this.st;
    }
    if (this.handle.ended?.()) this.settle(`the run ended at ${cursor}s before this step`);
    return this.st;
  }

  /**
   * Jump to the current step's timestamp rather than waiting for the run to get there. On a live
   * run, `scrubTo` clamps to what has been recorded so far: if the run has not reached `at_sim_s`
   * yet, the cursor lands short and the step stays advancing rather than settling somewhere earlier
   * than asked. `tick()` then does the rest, including its own arrived/overshot/ended handling.
   */
  skip(): StepState {
    if (!this.st.advancing) return this.st;
    this.handle.scrubTo(this.st.step.at_sim_s);
    this.tick();
    return this.st;
  }

  private async apply(set: Record<string, PatchValue>): Promise<string | undefined> {
    if (this.handle.mode === 'replay') return REPLAY_SET_REFUSED;
    try {
      await this.handle.update(set);
      return undefined;
    } catch (e) {
      return e instanceof Error ? e.message : String(e);
    }
  }

  private advance(step: WalkthroughStep): void {
    if (this.handle.mode === 'replay') {
      // Every frame already exists, so the timestamp is a seek, not a wait; tick() below settles it
      // (and handles the rare case where the seek itself overshoots).
      this.handle.scrubTo(step.at_sim_s);
    } else {
      this.handle.setSpeed(this.speedOf(step));
      this.handle.play();
    }
    this.tick();
  }

  private settle(reason?: string): void {
    this.handle.pause();
    this.st = {
      ...this.st,
      reason: reason ?? this.st.reason,
      advancing: false,
      paused: false,
      done: this.st.index === this.script.steps.length - 1,
    };
  }
}
