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

## 10. Hardware reference numbers

> **Provenance warning.** These are from published vendor specs and benchmark literature as
> of early 2026, reproduced from memory. Use them to get the *shape* right and the
> *ratios* right. Before you calibrate against them, verify any number you depend on against
> a primary source. Measured throughput in particular varies 2-3x with engine version,
> kernel choice, and batch composition.

### 10.1 Accelerator specs

Dense FLOPS below are **without** sparsity, which is what matters for inference.

| Accelerator | HBM | HBM bandwidth | BF16 dense | FP8 dense | FP4 dense |
|---|---|---|---|---|---|
| A100 80GB SXM | 80 GB HBM2e | 2.0 TB/s | 312 TF | n/a | n/a |
| H100 80GB SXM | 80 GB HBM3 | 3.35 TB/s | ~990 TF | ~1980 TF | n/a |
| H100 80GB PCIe | 80 GB HBM2e | 2.0 TB/s | ~760 TF | ~1500 TF | n/a |
| H200 141GB SXM | 141 GB HBM3e | 4.8 TB/s | ~990 TF | ~1980 TF | n/a |
| B200 | 192 GB HBM3e | 8.0 TB/s | ~2.2 PF | ~4.5 PF | ~9 PF |
| MI300X | 192 GB HBM3 | 5.3 TB/s | ~1300 TF | ~2600 TF | n/a |
| MI325X | 256 GB HBM3e | 6.0 TB/s | ~1300 TF | ~2600 TF | n/a |

Achievable fractions, which are the numbers you actually put in a config:

| Quantity | Symbol | Realistic range | Notes |
|---|---|---|---|
| Model FLOPS utilization, prefill | MFU | 0.35 – 0.60 | high end needs long prompts and large chunks |
| Memory bandwidth utilization, decode | MBU | 0.60 – 0.85 | falls sharply at batch 1 |
| Fixed per-step overhead | — | 0.5 – 5 ms | launch, sampling, scheduler; CUDA graphs cut this a lot |

The H200 is worth noting for a simulator: same compute as H100, 43% more bandwidth and 76%
more memory. Since decode is bandwidth-bound and capacity is KV-bound, it is substantially
better at serving while being identical at prefill. That asymmetry is a nice thing to
explore in a heterogeneous-fleet scenario.

### 10.2 The locality hierarchy

Bandwidth is per GPU unless stated. Unidirectional figures, since a KV transfer is one-way.

| Tier | Bandwidth | One-way latency | What lives here |
|---|---|---|---|
| HBM (on-package) | 2.0 – 8.0 TB/s | ~0.3 – 1 µs | weights, KV of running sequences, activations |
| NVLink 4, intra-node (H100) | ~450 GB/s | ~1 – 3 µs | TP collectives, GPU-to-GPU KV move |
| NVLink 5, intra-node (B200) | ~900 GB/s | ~1 – 3 µs | as above |
| NVLink domain, GB200 NVL72 | ~900 GB/s across 72 GPUs | ~2 – 4 µs | makes a rack look like one node |
| PCIe Gen5 x16 (GPU↔host) | ~64 GB/s | ~2 – 5 µs | KV swap to host DRAM, weight load |
| Host DRAM | ~300 – 500 GB/s per socket | ~0.1 µs | swapped KV, offloaded prefix cache |
| RDMA, 1 rail (NDR 400 Gb/s) | ~50 GB/s | ~2 – 5 µs | per-GPU NIC |
| RDMA, node aggregate (8 rails) | ~400 GB/s | ~2 – 5 µs | rail-optimized, 1 NIC per GPU |
| RDMA, XDR 800 Gb/s per rail | ~100 GB/s | ~2 – 5 µs | newer fabrics |
| Leaf-to-spine, same cluster | often 1:1 in AI fabrics | ~5 – 10 µs | oversubscription varies; check yours |
| Same-AZ TCP/IP | NIC-limited | ~100 – 500 µs | control plane, client traffic |
| Cross-region | link-limited | ~10 – 100 ms | geo routing only |
| Local NVMe Gen5 | ~7 – 14 GB/s per drive, ~50 GB/s per node | ~50 – 100 µs | weight cache, prefix cache spill |
| Networked / parallel filesystem | NIC-limited, 10 – 100 GB/s | ~0.5 – 2 ms | shared weight store |
| Object storage | ~1 – 10 GB/s with heavy parallelism | ~50 – 200 ms first byte | cold weight source |

Two orders of magnitude separate HBM from RDMA, and four separate HBM from object storage.
Every architectural choice in inference serving is about staying as high in this table as
possible.

### 10.3 Prefill throughput

For a dense transformer, ignoring attention's quadratic term:

```
flops_per_token  = 2 * n_params           # 2 FLOPs per parameter per token
prefill_tokens_s = MFU * peak_dense_flops * n_gpus / flops_per_token
```

Worked, H100 SXM, and compare against the measured column:

| Model | Precision | GPUs | FLOPs/token | Roofline at MFU 0.45 | Typically measured |
|---|---|---|---|---|---|
| Llama-3-8B | BF16 | 1 | 16 GF | ~28k tok/s | ~15 – 30k tok/s |
| Llama-3-8B | FP8 | 1 | 16 GF | ~56k tok/s | ~25 – 50k tok/s |
| Llama-3-70B | BF16 | 8 (TP8) | 140 GF | ~25k tok/s | ~8 – 20k tok/s |
| Llama-3-70B | FP8 | 8 (TP8) | 140 GF | ~51k tok/s | ~15 – 35k tok/s |
| Llama-3-405B | FP8 | 8 (TP8) | 810 GF | ~8.8k tok/s | ~4 – 8k tok/s |

Rules of thumb that fall out:

- Prefill throughput is roughly **inversely proportional to parameter count**, and roughly
  **linear in GPU count** until tensor-parallel communication eats the gain, which starts to
  bite past TP8 within a node and badly across nodes.
- A single H100 prefills a 70B model at only a few thousand tokens per second, which is why
  70B is never served on one GPU.
- Prefilling a 32k-token prompt on a 70B model at 15k tok/s takes ~2.1 s. That is the number
  behind the prefill-blocks-decode problem in section 4.

The quadratic attention term, if you need it:

```
attn_flops = 4 * n_layers * n_heads * head_dim * n^2     # roughly, for full causal attention
```

Compare that against `2 * n_params * n` and you find the crossover for 70B lands somewhere
around 16 – 32k tokens. Below that, ignore it. Above it, prefill cost grows superlinearly and
long-context requests get dramatically more expensive than a token count suggests.

### 10.4 Decode step time, and why reality is 2-3x the roofline

Per step the GPU must read all the weights, plus the KV of every sequence in the batch:

```
bytes_per_gpu = (weight_bytes + kv_bytes_per_token * sum_of_seq_lens) / n_gpus
t_step        = bytes_per_gpu / (HBM_bw * MBU) + t_fixed + t_collective
```

With tensor parallelism both weights and KV shard across ranks, so the division by `n_gpus`
applies to both. Add a collective term: TP does two all-reduces per layer, so
`t_collective` grows with layer count and shrinks with NVLink bandwidth.

Llama-3-70B, BF16, 8x H100, MBU 0.70, so 2.35 TB/s effective per GPU. Weights are 140 GB
total, 17.5 GB per GPU. KV is 320 KiB/token total, 40 KiB/token per GPU.

| Batch | Avg seq len | KV per GPU | Weight time | KV time | Roofline step | Realistic step | Per-seq ITL |
|---|---|---|---|---|---|---|---|
| 1 | 2k | 0.08 GB | 7.4 ms | 0.03 ms | 7.5 ms | ~15 – 25 ms | 15 – 25 ms |
| 16 | 4k | 2.6 GB | 7.4 ms | 1.1 ms | 8.5 ms | ~15 – 25 ms | 15 – 25 ms |
| 64 | 4k | 10.5 GB | 7.4 ms | 4.5 ms | 11.9 ms | ~18 – 30 ms | 18 – 30 ms |
| 128 | 4k | 21 GB | 7.4 ms | 8.9 ms | 16.3 ms | ~22 – 35 ms | 22 – 35 ms |
| 256 | 4k | 42 GB | 7.4 ms | 17.9 ms | 25.3 ms | ~32 – 50 ms | 32 – 50 ms |
| 64 | 32k | 84 GB | 7.4 ms | 35.7 ms | 43.1 ms | ~50 – 70 ms | 50 – 70 ms |

Read that table carefully, because four important things are visible in it:

1. **At batch 1 the roofline is wrong by 2-3x.** Measured single-stream decode for 70B on
   8x H100 is roughly 15-25 ms per token, not 7.5. The gap is kernel launch overhead, TP
   all-reduce latency, sampling, and Python scheduling. A simulator that omits `t_fixed`
   will overstate small-batch performance badly. Calibrate `t_fixed` against a batch-1
   measurement; it is the easiest number to get and the one that anchors everything.
2. **Batch 1 to 16 is nearly free.** Step time barely moves while throughput rises 16x.
   This is why continuous batching works, and why an idle replica is pure waste.
3. **KV read overtakes weight read.** Somewhere near batch 64-128 at 4k context, the KV term
   passes the weight term, and past that point step time grows roughly linearly with total
   tokens in the batch. That is the knee in your latency-versus-load curve.
4. **Context length and batch size are interchangeable in cost.** Batch 64 at 32k costs about
   the same per step as batch 512 at 4k. The correct state variable for a scheduler is
   **total KV tokens resident in the batch**, not batch size. This is the single most
   important modeling decision in the replica.

Throughput at the bottom of that table: batch 256 at 25 ms per step is ~10,200 output
tokens/s per replica. Batch 1 is ~50. Four hundred times. That ratio is why every dynamic
here is about keeping batches full without letting them get so full that SLOs break.

### 10.5 Where KV can live, and what it costs to move

**A sequence cannot decode unless its KV is in HBM.** Every other tier is a place to *park*
or *ship* KV, and the cost is a one-time transfer, not a per-step cost. Getting this
distinction right keeps the simulator honest.

One sequence, 4k tokens, Llama-3-70B: **1.31 GB** of KV. Time to move it:

| Path | Bandwidth | Time for 1.31 GB | Used for |
|---|---|---|---|
| Already in HBM | — | 0 | the only place decode happens |
| GPU→GPU, NVLink 4 | 450 GB/s | ~2.9 ms | intra-node P/D handoff, TP reshard |
| Node→node, 8 RDMA rails | 400 GB/s | ~3.3 ms | **P/D disaggregation** |
| Node→node, 1 RDMA rail | 50 GB/s | ~26 ms | P/D on a thin fabric: too slow |
| GPU→host DRAM, PCIe5 | 64 GB/s | ~20 ms | **preemption by swap** |
| GPU→local NVMe | 10 GB/s | ~131 ms | prefix cache spill |
| GPU→object storage | 2 GB/s | ~655 ms | not viable for KV |

The conclusions matter for policy design:

- **Prefill/decode disaggregation is only viable on multi-rail RDMA or NVLink.** At ~3 ms the
  transfer is a fraction of one decode step and disappears into the noise. At ~26 ms on a
  single rail it costs more than a whole step and the idea stops working. If your VISION
  includes disaggregation, the fabric assumption is load-bearing and belongs in the config.
- **Swap-based preemption costs ~20 ms out and ~20 ms back**, so ~40 ms round trip, versus
  recomputing the prefill, which for 4k tokens at 15k tok/s is ~270 ms. So swap is roughly
  7x cheaper than recompute *if* host DRAM has room and PCIe is not contended. Under heavy
  preemption PCIe becomes the bottleneck and that advantage collapses. Both are worth
  modeling as alternative policies.
- **Prefix cache offload to host DRAM is attractive**: a 20 ms load beats a 270 ms recompute.
  To NVMe at 131 ms it is marginal. To object storage it is pointless.
- With GB200 NVL72, 72 GPUs share one NVLink domain, so "intra-node" now means a whole rack.
  The 2.9 ms row applies rack-wide, which makes rack-scale disaggregation and KV pooling far
  more practical than on H100 clusters. Worth a scenario if you care about next-gen topology.

### 10.6 Cold start budget

For a 70B model, 140 GB of weights in BF16:

| Stage | Time | Notes |
|---|---|---|
| Node provisioning, if not pooled | 30 s – 5 min | cloud dependent, sometimes unavailable |
| Container image pull | 30 s – 5 min | tens of GB; usually cached |
| Weight read, local NVMe at 10 GB/s | ~14 s | best case |
| Weight read, parallel FS at 5 GB/s | ~28 s | common |
| Weight read, object storage at 2 GB/s | ~70 s | cold |
| Host→HBM over PCIe5 | ~2 s | rarely the bottleneck |
| CUDA graph capture, warmup, autotune | 10 – 60 s | engine dependent |
| **Total, warm node, cached weights** | **~30 – 60 s** | |
| **Total, cold node, cold weights** | **~3 – 10 min** | |

Set against a traffic burst that develops in 10-30 s, this is the whole argument of section 7.
Even the best case is slower than the burst. A warm pool of loaded-but-idle replicas is the
only thing that changes the picture, and it costs money to hold, which makes pool sizing a
genuine policy question with a real tradeoff.

### 10.7 Suggested starting config values

A concrete parameter set to seed the simulator, for Llama-3-70B BF16 on 8x H100 SXM:

```yaml
accelerator:
  name: h100-sxm-80
  count_per_replica: 8
  hbm_bytes_per_gpu: 80e9
  hbm_bandwidth_bytes_s: 3.35e12
  peak_dense_flops_bf16: 990e12
model:
  n_params: 70e9
  n_layers: 80
  n_kv_heads: 8
  head_dim: 128
  dtype_bytes: 2
  kv_bytes_per_token: 327680      # 2 * 80 * 8 * 128 * 2
  weight_bytes: 140e9
engine:
  mbu_decode: 0.70                # memory bandwidth utilization
  mfu_prefill: 0.45               # model FLOPS utilization
  fixed_step_overhead_s: 0.008    # dominates at low batch; calibrate first
  kv_block_tokens: 16
  kv_capacity_tokens: 1_370_000   # (640e9 - 140e9 - workspace) / 327680
  max_batched_tokens_per_step: 8192   # chunked prefill budget
  max_num_seqs: 256
fabric:
  nvlink_bytes_s: 450e9
  rdma_bytes_s_per_node: 400e9
  pcie_bytes_s: 64e9
  host_dram_bytes_s: 400e9
  local_nvme_bytes_s: 10e9
lifecycle:
  cold_start_s: 45                # warm node, cached weights
  cold_start_s_cold_node: 300
```

Sanity checks this config should reproduce, and good first tests:

- Batch 1 decode: ~15 ms per token, so ~65 tokens/s.
- Batch 256 at 4k context: ~25-32 ms per step, so ~8-10k output tokens/s per replica.
- Prefill of a 32k prompt: ~2 s if run as one step, which is why the chunk budget exists.
- KV exhaustion at roughly 334 concurrent sequences averaging 4k tokens.
- 8192-token step budget means a 32k prompt takes 4 chunked steps, adding ~4 steps of ITL
  delay to everyone else in the batch instead of a single 2 s stall.

---

## 11. A tractable cost model for the simulator

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

## 12. Things you can safely ignore in v1

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

## 13. Glossary

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
