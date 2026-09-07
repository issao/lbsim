//! What a replica did to a traced sequence, step by step.
//!
//! `Replica::step` calls into the `Tracer` at five points: admission, each prefill chunk, once when
//! the step's duration is known, each decode step, and retirement. The tracer ignores every id it was
//! not told to track, so with tracing off each call is one branch on an empty list, and the loop in
//! `sim-leaf` decides which ids to track and turns what it reads back into `sim_metrics` spans. The
//! split keeps this crate typed on `sim-core` alone: the physics does not know what a trace is for.
//!
//! Timing is the step's, not the event's. Every event inside a step spans the whole step, because a
//! step is the engine's unit of time: a prefill chunk or a decode token is not done until the step
//! ends, and a sequence admitted at the top of a step first computes in that step. So events record
//! which step they belong to and take their times from that step's snapshot.

use sim_core::Nanos;

/// What the replica was doing during one step, recorded once per step while any traced sequence is
/// on the replica. `kv_tokens` is the resident total going into the step, before this step's decode
/// tokens are added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub start: Nanos,
    pub end: Nanos,
    /// Sequences in the step, prefilling or decoding.
    pub batch_size: u32,
    /// Sequences on the replica, which is the same set today: nothing admitted sits out a step.
    pub running: u32,
    /// Sequences still waiting in the queue after this step's admission.
    pub queued: u32,
    pub kv_tokens: u64,
    pub decoding: u32,
    pub prefill_tokens: u32,
    pub step_ns: Nanos,
}

/// One thing that happened to one traced sequence, with the step's times filled in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepEvent {
    /// Left the queue and joined the batch at `at`, the start of its first step.
    Admitted { id: u64, at: Nanos },
    /// `tokens` of prompt processed in the step from `start` to `end`.
    PrefillChunk { id: u64, tokens: u32, start: Nanos, end: Nanos },
    /// One token, emitted at `end`.
    DecodeStep { id: u64, start: Nanos, end: Nanos },
    /// Emitted its last token at `at`.
    Retired { id: u64, at: Nanos },
}

impl StepEvent {
    pub fn id(&self) -> u64 {
        match *self {
            StepEvent::Admitted { id, .. }
            | StepEvent::PrefillChunk { id, .. }
            | StepEvent::DecodeStep { id, .. }
            | StepEvent::Retired { id, .. } => id,
        }
    }
}

#[derive(Clone, Copy)]
enum Raw {
    Admitted,
    Prefill(u32),
    Decode,
    Retired,
}

/// Per-replica recorder. Tracks a handful of ids at most, so a linear scan beats a hash set.
#[derive(Default)]
pub struct Tracer {
    tracked: Vec<u64>,
    /// `(id, what, step)`: the index into `steps` of the snapshot the event belongs to.
    events: Vec<(u64, Raw, usize)>,
    steps: Vec<ResourceSnapshot>,
}

impl Tracer {
    /// Start recording `id`. Called when the request is enqueued, before its first step.
    pub fn track(&mut self, id: u64) {
        if !self.tracked.contains(&id) {
            self.tracked.push(id);
        }
    }

    #[inline]
    fn tracks(&self, id: u64) -> bool {
        !self.tracked.is_empty() && self.tracked.contains(&id)
    }

    /// Admission happens before the step's snapshot exists, so it belongs to the snapshot about to be
    /// recorded, as does every prefill chunk.
    #[inline]
    pub fn admitted(&mut self, id: u64) {
        if self.tracks(id) {
            self.events.push((id, Raw::Admitted, self.steps.len()));
        }
    }

    #[inline]
    pub fn prefill_chunk(&mut self, id: u64, tokens: u32) {
        if self.tracks(id) {
            self.events.push((id, Raw::Prefill(tokens), self.steps.len()));
        }
    }

    /// Once per step, after the step's duration is known and before its decode tokens are attributed.
    #[inline]
    pub fn snapshot(&mut self, snap: ResourceSnapshot) {
        if !self.tracked.is_empty() {
            self.steps.push(snap);
        }
    }

    /// Decode and retirement come after the snapshot, so they belong to the last one recorded.
    #[inline]
    pub fn decode_step(&mut self, id: u64) {
        if self.tracks(id) {
            self.events.push((id, Raw::Decode, self.steps.len().saturating_sub(1)));
        }
    }

    #[inline]
    pub fn retired(&mut self, id: u64) {
        if self.tracks(id) {
            self.events.push((id, Raw::Retired, self.steps.len().saturating_sub(1)));
        }
    }

    /// Everything recorded for `id`, in order, with the step each event ran in; and stop tracking it.
    /// Snapshots below the oldest step any remaining event still points at are dropped and the
    /// survivors rebased onto the shrunk vector, so a continuously busy replica carries only the span
    /// of steps its still-tracked requests actually cover, not the whole run's history.
    pub fn take(&mut self, id: u64) -> Vec<(StepEvent, ResourceSnapshot)> {
        let mut out = Vec::new();
        if let Some(pos) = self.tracked.iter().position(|&x| x == id) {
            self.tracked.swap_remove(pos);
        }
        self.events.retain(|&(eid, raw, step)| {
            if eid != id {
                return true;
            }
            let snap = self.steps.get(step).copied().unwrap_or_default();
            let ev = match raw {
                Raw::Admitted => StepEvent::Admitted { id, at: snap.start },
                Raw::Prefill(tokens) => StepEvent::PrefillChunk { id, tokens, start: snap.start, end: snap.end },
                Raw::Decode => StepEvent::DecodeStep { id, start: snap.start, end: snap.end },
                Raw::Retired => StepEvent::Retired { id, at: snap.end },
            };
            out.push((ev, snap));
            false
        });
        let cut = self.events.iter().map(|&(_, _, step)| step).min().unwrap_or(self.steps.len());
        if cut > 0 {
            self.steps.drain(0..cut);
            for ev in &mut self.events {
                ev.2 -= cut;
            }
        }
        out
    }

    pub fn tracking(&self) -> usize {
        self.tracked.len()
    }

    /// Snapshots currently held. Test-only window into the growth `take` is meant to bound.
    #[doc(hidden)]
    pub fn snapshot_len(&self) -> usize {
        self.steps.len()
    }
}
