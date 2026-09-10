//! The scheduling seam, re-exported from `sim_core::scheduling`, where it is defined below both the
//! engine that calls it and the policies that implement it (`docs/ARCHITECTURE.md` section 10.8 lets
//! neither reach the other). A scheduling policy file `use`s it from here like the other seams.

pub use sim_core::scheduling::{
    fill_buffer, pick_max_by, BufferLimits, Fill, QueuedWork, SchedulingPolicy, SeqView, StepView, Victim,
};
