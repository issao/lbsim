# LLM inference serving: the mechanics that drive load dynamics

Written for a distributed systems engineer who has not worked inside an inference engine.
Purpose: give you the vocabulary and the causal chains, so VISION.md can name specific
phenomena rather than "realistic load."

Numbers marked ~ are order-of-magnitude and must be calibrated, not trusted.

---

## 1. A request has two phases with opposite resource profiles

**Prefill** processes the whole prompt at once to produce the first output token.
All prompt tokens go through the model in parallel, so the GPU is doing large matrix
multiplies. It is **compute-bound**. Cost scales with prompt length.

**Decode** produces one token per forward pass, autoregressively. Batch size is the number
of concurrent sequences, which is small compared to prompt length, so the matrix multiplies
are skinny. The GPU spends its time *reading the model weights out of HBM*, not computing.
It is **memory-bandwidth-bound**. Cost per step is nearly independent of how many sequences
are in the batch, until the batch gets large.

That asymmetry is the single most important fact in this domain. Almost every dynamic worth
simulating falls out of it.

Consequences:

- Throughput comes from batching decode. One sequence decoding alone wastes ~99% of the GPU.
- Prefill does not benefit much from batching; it is already saturating compute.
- The two phases interfere. See section 4.

### Latency metrics that follow from the phases

| Metric | Meaning | Driven by |
|---|---|---|
| TTFT | time to first token | queueing + prefill |
| ITL / TPOT | inter-token latency, time per output token | decode step time |
| E2E | total request latency | TTFT + ITL x output_len |

Users perceive these differently. TTFT is "did it hang." ITL is "is it typing at a readable
speed." A chat product needs ITL below roughly human reading speed, maybe ~30-50 ms/token,
and TTFT under ~500 ms. A batch summarization job cares about neither, only throughput.
This is why SLO classes are not decoration; they change the optimal policy.

**Goodput**, not throughput, is the real objective: tokens per second *delivered within SLO*.
A server can have excellent throughput and near-zero goodput by making everyone slightly
too slow. Any policy evaluation that reports only throughput will pick the wrong policy.

---

## 2. Continuous batching is the scheduler you are modeling

Naive batching waits for a batch to fill, runs it to completion, then starts the next. A
short request stuck behind a long one waits for the long one to finish. Terrible.

Real engines (vLLM, SGLang, TensorRT-LLM) use **continuous batching**, also called
iteration-level scheduling, from the Orca paper. The engine loop is:

```
every step:
    admit new requests from the waiting queue, if capacity allows
    run ONE forward pass over the current batch
    emit one token for each decoding sequence
    retire sequences that hit EOS or max_tokens
```

So the batch composition changes every ~10-50 ms. A finished sequence frees its slot
immediately and a waiting request takes it. This is the mechanism the simulator must
reproduce faithfully, because it is where the interesting queueing behavior lives.

Note what this means architecturally: **the replica is itself a scheduler**, with its own
admission decision every step. Your router and your replica are two scheduling layers, and
they can fight each other.

---

## 3. KV cache is the binding capacity constraint

Each token, once processed, leaves behind key and value tensors that every future token
attends to. That is the KV cache. It lives in HBM and it is what actually limits concurrency.

```
kv_bytes_per_token = 2 * n_layers * n_kv_heads * head_dim * dtype_bytes
```

The leading 2 is K and V. Worked examples, fp16:

| Model | layers | kv_heads | head_dim | bytes/token |
|---|---|---|---|---|
| Llama-3-8B | 32 | 8 | 128 | ~128 KiB |
| Llama-3-70B | 80 | 8 | 128 | ~320 KiB |

On 8xH100 (640 GB HBM) serving 70B in fp16: weights ~140 GB, activations and workspace
some more, leaving ~450 GB for KV. That is ~1.4M tokens of KV cache. At an average of
4k tokens per sequence, roughly 350 concurrent sequences. That number is your replica's
real capacity, and it is a *token* budget, not a *request* budget.

Two requests are not equal. A 100k-token context request consumes as much KV as 25 chat
turns. Any policy that counts requests is measuring the wrong thing.

**PagedAttention** (vLLM's contribution) stores KV in fixed-size blocks, typically 16 tokens,
in a page table, instead of one contiguous reservation per sequence. This removes internal
fragmentation from over-reserving for unknown output length. For a simulator, the effect is
that you can model KV as a simple token counter with block-granularity rounding, and you do
not need to model fragmentation as a first-order concern.

### When KV runs out: preemption

Output length is unknown in advance. The engine admits a request, it decodes longer than
expected, and the cache fills. The engine must then **preempt** a running sequence:

- **Recompute**: drop its KV, put it back in the queue, redo prefill later. Wastes the
  prefill compute, cheap in memory.
- **Swap**: copy its KV to host memory, copy back later. Costs PCIe bandwidth, ~64 GB/s,
  and a 4k-token 70B sequence is ~1.3 GB, so ~20 ms each way. Not free.

Preemption is the origin of one of the nastiest dynamics: a load spike fills KV, preemption
starts, preempted requests re-enter the queue and demand prefill again, which consumes the
compute needed to drain the queue. **This is a positive feedback loop.** It looks like a
throughput collapse under load, and it is a metastable failure: the system stays collapsed
after the offered load returns to normal. If your simulator reproduces one thing, make it
this one.

---

## 4. Prefill and decode interfere, and this is the main source of latency spikes

A step can do prefill work or decode work. If a new request with a 32k-token prompt arrives
and the engine runs its prefill as one step, that step takes maybe ~1-2 s. Every sequence
already decoding in that batch sees a ~1-2 s gap between tokens. One arrival wrecked ITL for
fifty unrelated users.

Mitigations, all of which are policy choices worth simulating:

- **Chunked prefill**: split the prefill into chunks of C tokens, e.g. 512, and give each
  step a mixed token budget, some prefill and some decode. Bounds the ITL spike at the cost
  of higher TTFT for the long request. The knob is the token budget per step.
- **Prefill prioritization vs decode prioritization**: which gets the budget under pressure.
  Favoring prefill improves TTFT and hurts ITL, and increases KV pressure because you admit
  faster. Favoring decode does the reverse and can starve arrivals.
- **Disaggregation**: run prefill on one pool of GPUs and decode on another, shipping the KV
  cache over the network between them. Removes the interference entirely. Costs a KV transfer,
  ~1.3 GB for the example above, so it needs fast interconnect, and it adds a hop plus a new
  failure mode plus a new load-balancing problem (two pools to size independently). Real
  systems do this now. It is a strong candidate for a policy you evaluate.

---

## 5. Prefix caching turns routing into a locality problem

If two requests share a prefix, the system prompt, a few-shot preamble, a conversation
history, the KV for that prefix can be reused and the prefill skipped. vLLM calls it
automatic prefix caching; SGLang organizes it as a radix tree over cached prefixes.

The hit rate can be very high in practice. Chat sessions re-send the whole history each turn,
so turn N reuses everything from turn N-1. A shared system prompt is reused across all users
of an application.

This creates a direct tension your simulator should expose:

- **Load balancing** wants to spread requests evenly across replicas.
- **Prefix locality** wants to send a request to the replica that already holds its prefix.

Naive least-loaded routing destroys hit rate. Naive affinity routing creates hot spots and
ignores that a replica's cache is finite and evicts under pressure. The right answer is
somewhere between, and it depends on the workload's sharing structure. That is exactly the
kind of question a simulator answers cheaply and a production A/B test answers expensively.

Modeling requirement: requests need a **prefix identity**, not just a token count. Sessions
with a shared history, and applications with a shared system prompt, are the two structures
to generate.

---

## 6. Why standard load balancing fails here

Compared to a stateless web service, an inference replica is unusual:

- Service time varies by 100x or more, from a 20-token completion to a 4000-token one, and
  it is **not known at arrival**. Output length is the hidden variable.
- The replica holds **per-request state** (KV) for the whole life of the request, so capacity
  is memory, not concurrency slots.
- Requests are **long-lived**, seconds to minutes, so a routing mistake is not quickly washed
  out by the next request. Compare a 5 ms HTTP request.
- Replicas have **warm state** (prefix cache) that makes them non-interchangeable.

Therefore:

- **Round-robin** ignores that requests are unequal. One replica gets three long-context
  requests and melts.
- **Least-connections / least-requests** counts requests, but the constraint is KV tokens
  and queue-time-to-drain. Better than round-robin, still wrong.
- **Least-loaded on a stale metric** is actively dangerous. Because requests are long-lived
  and metrics scrape on an interval, ~1-15 s, every router sees the same stale "that replica
  is idle" and stampedes it. This is herding, and it is worse the more routers you have.
- Things that work better: routing on **queue depth plus KV utilization** reported by the
  replica; **power-of-two-choices**, which is remarkably robust to stale information;
  **estimated-work** routing using prompt length as a proxy; prefix-affinity with a load cap.

There is a large design space here and it is your highest-value experiment area.

---

## 7. Autoscaling is dominated by cold start

Adding a replica is not fast. You must schedule a pod onto a GPU node, possibly wait for a
node to be provisioned, pull a container image of tens of GB, load model weights of tens to
hundreds of GB into HBM, and then warm up, CUDA graph capture and compile. Realistically
~1-5 minutes for a large model, worse from cold object storage, much better with local NVMe
and a warm pool.

Compare that to the timescale of a traffic burst, which can be tens of seconds. The
implication is stark: **reactive autoscaling cannot catch a burst.** By the time capacity
arrives, either the burst is over or the system has already collapsed. Real answers are
predictive scaling on diurnal patterns, warm pools of pre-loaded replicas, headroom targets
that look wasteful, and admission control to survive the gap.

Also: scaling on GPU utilization is a trap. A decode-bound replica shows high utilization
while being memory-bandwidth-bound, and utilization stays pinned high across a wide range of
actual load. Scale on queue depth, TTFT, or KV utilization instead.

Scale-down has its own hazard. Draining a replica means waiting for in-flight requests, which
can be minutes for long generations, and you lose its prefix cache, so the remaining replicas
see a hit-rate drop and an effective capacity drop right when you removed capacity.

---

## 8. Failure and overload modes to reproduce

- **Retry storms.** A client timeout fires, it retries, offered load rises exactly when the
  server is struggling. Without a retry budget or circuit breaker this is a classic
  metastable collapse. LLM requests make it worse because a retry after a 30 s timeout has
  usually already consumed 30 s of GPU work, so retries cost far more than in a web service.
- **Head-of-line blocking across tenants.** One tenant sends 100k-token prompts and fills KV.
  Every other tenant's latency degrades. Fairness needs per-tenant token accounting, not
  request counting.
- **Stragglers.** One replica is slow, thermal throttling, a noisy neighbor, a degraded NVLink,
  a partially failed collective. Load balancers that route on stale metrics send it *more*
  work. Detecting and ejecting a slow replica is a policy worth testing.
- **Partial failure.** With tensor parallelism, all GPUs in a replica are one failure domain,
  so a single GPU fault kills the whole replica and all its in-flight requests.
- **Cascading overload after a partial outage.** Lose 20% of replicas, the remaining 80% take
  125% of load, they slow down, clients time out and retry, and the survivors collapse too.
- **Queue timeouts and wasted work.** A request that has waited past its deadline should be
  dropped *before* it consumes GPU time, not after. Systems that do not do this spend their
  capacity generating tokens nobody will read. Under overload this is most of the capacity.

---

## 9. Admission control and traffic shaping

Because a request's cost is roughly proportional to tokens, not requests, the correct unit of
rate limiting is tokens. A token bucket on requests per second lets a single client consume
the fleet.

The tricky part is that **cost is unknown at admission**. Prompt length you know. Output
length you do not. Options: charge for max_tokens up front and refund, predict output length
from a learned model, or charge continuously as tokens are produced. Each has different
failure behavior under adversarial or bimodal workloads, and this is a nice thing to simulate.

Other shaping levers: priority queues by SLO class, deadline-aware scheduling with earliest
deadline first, dropping or degrading (shorter max_tokens, smaller model, quantized fallback)
under pressure, and per-tenant weighted fair queueing on tokens.

---

## 10. A tractable cost model for the simulator

You do not need to model kernels. A roofline approximation gets the shape right, which is
what matters for policy comparison.

**Decode step**, memory-bound:

```
bytes_read = weight_bytes + sum_over_batch(kv_bytes_per_token * seq_len)
t_step     = bytes_read / effective_bw + fixed_overhead
```

`effective_bw` is maybe 60-80% of spec HBM bandwidth. `fixed_overhead` is kernel launch and
Python overhead, ~1-5 ms, and it matters at small batch sizes. This formula reproduces the
essential curve: throughput rises with batch size, ITL degrades with batch size and with the
total KV in the batch, so a batch of long contexts is slower per step than a batch of short ones.

**Prefill**, compute-bound:

```
flops   = 2 * n_params * n_prefill_tokens  +  attention_term(n)
t_prefill = flops / effective_flops
```

`effective_flops` is maybe 40-70% of peak dense FLOPS. The attention term is quadratic in
sequence length and only matters above roughly ~8-16k tokens, so include it if long context
is in scope.

**Mixed step** with chunked prefill: one step has a token budget, split between prefill chunks
and decode tokens, and its time is roughly the max of the compute-bound and bandwidth-bound
estimates, or a sum if you want to be pessimistic.

Calibrate against published vLLM or SGLang benchmark numbers for one model and GPU pair. The
goal is not absolute accuracy; it is that the *ordering* of policies is correct and the
*shape* of the latency-vs-load curve matches, including where the knee falls.

---

## 11. Things you can safely ignore in v1

- GPU kernel scheduling, warp occupancy, anything below the step.
- Network packets. Model a hop as a latency distribution.
- Tokenization cost, negligible.
- Weight quantization details, unless you want to compare quantized fallback as a policy;
  then it is just different weight_bytes and effective_flops.
- Multi-region, unless geographic routing is a goal.
- Speculative decoding, which makes tokens-per-step variable. Interesting later, and it
  interacts with batching in a non-obvious way: acceptance rate falls as batch size rises,
  because the bottleneck shifts.
- Mixture-of-experts routing imbalance, real but second-order for policy comparison, and it
  needs expert-parallel modeling to be meaningful.

---

## 12. Glossary

- **TTFT / ITL / TPOT / E2E** — see section 1.
- **Goodput** — throughput delivered within SLO.
- **Continuous batching / iteration-level scheduling** — section 2.
- **KV cache** — per-token attention state, the capacity constraint, section 3.
- **PagedAttention** — block-based KV allocation.
- **Prefix caching / radix attention** — reuse of KV for shared prefixes, section 5.
- **Chunked prefill** — splitting prefill across steps to bound ITL, section 4.
- **P/D disaggregation** — separate prefill and decode pools, section 4.
- **TP / PP / EP** — tensor, pipeline, expert parallelism. TP splits a layer across GPUs and
  needs fast interconnect; PP splits layers across stages and adds bubbles; EP splits MoE
  experts. TP group is one failure domain.
- **Preemption (recompute vs swap)** — evicting a running sequence when KV fills, section 3.
