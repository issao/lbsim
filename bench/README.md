# bench

Measurements that verify the design in `docs/ARCHITECTURE.md`. **Not** part of the product:
nothing here shares code with the simulator, and all of it can be deleted without loss.

Each item exists to answer one question that could change the architecture.

## `validate_epochs.py`

Question: is the closed-form epoch advance in `docs/ARCHITECTURE.md` section 3 exactly
equivalent to iterating every decode step, or does it approximate?

Answer: exactly equivalent, proven in rational arithmetic, and 82x fewer iterations on the
cases tested. Also more numerically accurate than the loop, since it does not accumulate
rounding over n additions.

```
python3 bench/validate_epochs.py
```

Exits non-zero if the closed form and the brute-force simulation ever disagree, so it is
suitable for CI. It also reproduces the decode step-time table in
`docs/llm-serving-primer.md` section 10.4, which cross-checks the hand-computed numbers there
against the algebra here.

## `queue/`

Question: can the event queue sustain the ~100 ns/event that the scale budget assumes?

Answer: yes, at 54 ns for a 6,250-entry `std::collections::BinaryHeap`, but only if the heap
stays small. At 1.7 million entries it costs 441 ns, which is 4.4x over budget. Heap size
dominates arity and implementation, and hand-rolled d-ary heaps were slower than the standard
library at every size tested.

```
cd bench/queue && cargo run --release
```

Needs the toolchain in `docs/toolchain.md`. Release build with link-time optimisation; a debug
build is meaningless for this measurement.
