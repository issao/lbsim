# Workload calibration reference

Companion to `docs/llm-serving-primer.md`. That document covers **hardware**: accelerator
specs, the locality hierarchy, prefill throughput, decode step time, KV transfer cost, cold
start (its section 10). This document covers **workload** — arrivals, lengths, sessions,
prefix sharing — plus the handful of **engine-efficiency constants** that turn a roofline
into a believable replica model.

The purpose is calibration, so every number carries a source and a date. Read the
provenance tags before you depend on anything.

## How to read the provenance tags

| Tag | Meaning |
|---|---|
| **[measured-here]** | Computed by us directly from the primary dataset. Reproducible: the scripts are trivial and the data is a `curl` away. Highest confidence in this document. |
| **[primary]** | Quoted from a paper, official doc, dataset README, or vendor blog that we fetched and read ourselves. |
| **[primary via research pass]** | Quoted from a primary source that a delegated research pass fetched and read, with the URL and date recorded, but that the author of this file did not personally re-open. Verify before quoting verbatim in anything that matters. |
| **[secondary]** | Reported by a source that is itself citing someone else, or a claim we could not re-read at the primary. Treat as a hint, not a fact. |
| **[inferred]** | Arithmetic or reasoning *we* did on top of a cited number. The input is sourced; the conclusion is ours. |
| **[GUESS]** | Nobody publishes this. A labelled default with a range. Do not cite it; sweep it. |
| **not found** | We looked and did not find it. Said explicitly rather than filled in. |

**The one-line summary.** Exactly one public trace lets you replay a *fleet* arrival process
with prefix-sharing structure (Mooncake, 1 hour). Exactly two let you replay a *fleet*
arrival process at multi-day scale with no sharing structure (Azure 2024, BurstGPT). Exactly
one gives you real agentic session structure with per-request cache accounting but only 43
users (TraceLab). Everything else is a length distribution with no clock.

## Where this document corrects the primer

Seven places. Each is argued in the section named.

| Primer says | Correction | Where |
|---|---|---|
| §12: speculative decoding — *"acceptance rate falls as batch size rises, because the bottleneck shifts"* | The **mechanism is stated the wrong way round.** Acceptance rate is a property of the (draft, target, prompt) triple and is batch-invariant; what falls is the **speedup**, because the extra verification FLOPs stop being free once the step is compute-bound. And at long context the bottleneck does *not* shift to compute, so speedup can *improve* with batch size — MagicDec measures up to 2.51× at batch 32–256. | §7.3 |
| §10.4: measured batch-1 decode for a 70B on 8×H100 is *"roughly 15-25 ms per token"*, a 2–3× gap over the 7.5 ms roofline | **Pessimistic by about 2× for a 2026 engine.** NVIDIA NIM measures **10.25 ms / 97.06 tok/s** for Llama-3.3-70B bf16 TP8 on 8×H100 — a gap of only ~1.4×, implying `t_fixed` ≈ 2.7 ms rather than 8 ms. The primer's figure is right for a 2024-era Python-scheduler engine (vLLM ≤ 0.5.3 spent 62 % of its step on CPU) and wrong for one with CUDA graphs plus a custom all-reduce. This moves the entire low-batch region of the latency-vs-load curve. | §6.1, §6.5 |
| §10.1: MBU range 0.60–0.85, *"falls sharply at batch 1"* | The direction is right but the magnitude is badly off, **and the primer's own §10.7 config contradicts it.** Measured effective batch-1 MBU on H100 is **~0.26** (median over 12 model/context cells), and it *rises* to 0.70–0.77 at large batch. Worse, pairing `mbu_decode: 0.70` with `fixed_step_overhead_s: 0.008` **double-counts the same overhead**. Pick a convention and say which. | §6.2 |
| §10.7: `mfu_prefill: 0.45` as a single value | Dense models reach **0.50–0.63** at long prompts on H100 (Llama-3-405B 63 %, Llama-3-70B 50–60 %, Llama-3.1-70B 56.6 %); **MoE models reach only 0.16–0.36**. A 2–3× architecture split that one constant cannot represent. Also: fp8 does *not* double prefill throughput — the GEMM curve tops out near 56–58 % of the doubled peak, so ~1.4×. | §6.3 |
| §5: *"The hit rate can be very high in practice"* for prefix caching | True for coding agents (95.7 % measured) and misleading for everything else. Kimi's own production ceiling with an **infinite** cache is *"up to only 50 %"*, and we independently compute **36.6 %** (conversation) and **55.3 %** (tool/agent) from their released block hashes. At the primer's own §10.7 KV capacity of 1.37 M tokens, the achievable conversation hit rate is about **6 %**. | §5.3, §5.4 |
| §5: prefix sharing framed as *"sessions with a shared history, and applications with a shared system prompt"* — as two co-equal structures | For API traffic the **shared system prompt dominates outright**. Aliyun's business trace has a multi-turn ratio below 0.1 % yet single-turn requests contribute **97 % of all cache hits**. The Mooncake conversation trace has exactly **one** distinct root block across 12,031 requests. Weight the system-prompt mechanism far more heavily than session chains. | §4.2, §5.4 |

Two places where the primer is **confirmed** and worth saying so:

- §6's claim that service time varies by 100× or more. Azure 2024 code has output p50 = 8 and
  max = 5,000 tokens (625×) on top of a 320× input spread, with an output-token CV of **3.30**.
  The primer understates it if anything.
- §10.7's sanity check of *"~8-10k output tokens/s per replica"* for a 70B at batch 256.
  TensorRT-LLM measures **8,773 tok/s** at 2048/2048 and **11,082** at 1000/1000 for
  Llama-3.1-70B FP8 on 8×H100 under infinite request rate. Dead on.

---

## 1. Public LLM inference traces

### 1.1 The table that matters: what can actually be replayed

| Trace | Arrival timestamps | Input tok | Output tok | Session id | Prefix structure | Model id | Span | Requests | Replayable as-is? |
|---|---|---|---|---|---|---|---|---|---|
| **Azure LLM 2024** (code, conv) | yes, µs, absolute | yes | yes | no | **no** | no (2 services) | 7 days each | 16.80 M + 27.30 M | **Yes — arrivals + lengths only** |
| **Azure LLM 2023** (code, conv) | yes, 100 ns | yes | yes | no | no | no | ~57 min each | 8,819 + 19,366 | Yes, but tiny |
| **Azure LMM 2025** (multimodal) | yes | yes (+ image count) | yes | no | no | no | 7 days | not measured here | Yes |
| **BurstGPT_3** | yes, 1 s resolution | yes | yes | **yes** (conv only) | no | ChatGPT / GPT-4 | 110 days | 5.34 M | **Yes — plus session structure** |
| **BurstGPT_1 / _2** | yes, 1 s | yes | yes | no | no | ChatGPT / GPT-4 | 121 days | 1.43 M + 3.86 M | Yes |
| **Mooncake FAST'25** (conv, toolagent) | yes, ms, relative | yes | yes | implicit via hashes | **yes — 512-tok block hashes** | no (Kimi) | 1 hour each | 12,031 + 23,608 | **Yes — the only one with sharing** |
| **Mooncake synthetic** | yes (Poisson-generated) | yes | yes | implicit | yes | no | 17 min | 3,993 | Arrivals are synthetic |
| **TraceLab** | yes, absolute ms, per event | yes (+ prefix/append split) | yes | **yes** | **yes — cache-read/create token counts** | yes (23+ versions) | Sep 2025 – Jun 2026 | 357,161 rounds | **Yes for sessions; no for fleet arrivals** (43 users) |
| **LMSYS-Chat-1M** | **no** | text (tokenize yourself) | text | conversation id | reconstructable from text | yes (25 models) | Apr–Aug 2023 | 1.0 M conversations | **No — lengths only** |
| **WildChat-1M** | **yes** (per conversation) | text | text | conversation hash | reconstructable | yes | Apr 2023 – Apr 2024 | ~1.04 M conversations | Partly — see 1.5 |
| **ShareGPT (V3 cleaned)** | **no** | text | text | conversation id | reconstructable | no | unknown | ~53 k conversations | **No — lengths only** |
| **ServeGen** (Alibaba) | generator, not a trace | parameterized | parameterized | partly | **no** (declined) | 12 model classes | Jan–Apr 2025 char. | 3.54 B characterized | It is a *generator*, not a replay trace |
| **FineServe** | claimed present | yes | yes | not stated | no | 57 models | 4 months | 1.48 B | Not verified by us |
| **Chutes 1-year trace** | claimed | yes | yes (+ `cached_tokens`) | not stated | per-request cached tokens | 9,174 models | Apr 2025 – Apr 2026 | 6.12 B | **Release promised, not verified available** |

### 1.2 Azure LLM inference traces (Microsoft) — the arrival-process workhorse

Repo: <https://github.com/Azure/AzurePublicDataset>. License: **CC-BY Attribution**
([LICENSE](https://github.com/Azure/AzurePublicDataset/blob/master/LICENSE)) — the cleanest
license of any trace here. [primary, fetched 2026-09-06]

Three LLM/LMM datasets exist:

| Dataset | Collected | Paper | Schema |
|---|---|---|---|
| `AzureLLMInferenceDataset2023` | doc says Nov 11 2023; **data timestamps say 2023-11-16** | Splitwise, ISCA 2024 ([arXiv 2311.18677](https://arxiv.org/abs/2311.18677)) | `TIMESTAMP, ContextTokens, GeneratedTokens` |
| `AzureLLMInferenceDataset2024` | May 10–19 2024 (files span exactly 168 h) | DynamoLLM, HPCA 2025 ([arXiv 2408.00741](https://arxiv.org/abs/2408.00741)) | same |
| `AzureLMMInferenceDataset2025` | Oct 15–22 2024 | ModServe, SoCC 2025 | `TIMESTAMP, NumImages, ContextTokens, GeneratedTokens` |

Two services in each LLM release: **Code** (inline code completion) and **Conversation**
(chat).

**What it does *not* have, and this is the important part:** no session id, no user id, no
model id, and **no prompt content or prefix identity**. Splitwise states it explicitly:
*"For this characterization, we do not reuse the KV-cache between requests to emulate a
cloud service with security guarantees."* [primary] So **never cite the Azure traces for a
prefix-sharing figure**. They calibrate arrivals and lengths, nothing else.

**Also important:** the 2024 traces are **hard-capped at 8k context**. Max `ContextTokens`
is 7,743 (code) and 7,999 (conv); the fraction above 8,000 is exactly zero.
[measured-here] These traces describe a 2024-era 8k-window deployment and **cannot** be used
to calibrate long-context or agentic traffic. Use Mooncake or TraceLab for that.

Statistics we computed directly from the released CSVs (full files, not samples):

| | 2023 code | 2023 conv | 2024 code | 2024 conv |
|---|---|---|---|---|
| Requests | 8,819 | 19,366 | **16,803,695** | **27,303,999** |
| Span | 0.95 h | 0.97 h | 168.00 h | 168.00 h |
| Mean rate | 2.57 req/s | 5.53 req/s | 27.78 req/s | 45.15 req/s |
| **Input mean / p50 / p90 / p99 / max** | 2,048 / 1,469 / 5,194 / 7,436 / 7,437 | 1,155 / 1,020 / 2,735 / 4,142 / 14,050 | **2,511 / 1,930 / 6,251 / 7,685 / 7,743** | **1,632 / 928 / 3,830 / 6,683 / 7,999** |
| Input CV | 0.96 | 0.96 | 0.85 | 0.94 |
| **Output mean / p50 / p90 / p99 / max** | 27.9 / 13 / 55 / 252 / 1,899 | 211 / 129 / 424 / 601 / 1,000 | **22.7 / 8 / 43 / 271 / 5,000** | **105.5 / 41 / 342 / 694 / 1,500** |
| Output CV | 2.15 | 0.77 | **3.30** | **1.50** |
| corr(input, output) | 0.001 | −0.112 | 0.000 | **+0.392** |
| Input:output, ratio of means | 73:1 | 5.5:1 | **111:1** | **15.5:1** |
| Frac output ≤ 10 tok | 0.365 | 0.000 | 0.579 | 0.187 |

[all measured-here, 2026-09-06, from the released CSVs]

Cross-check against the literature: Splitwise reports median prompt 1,500 (code) / 1,020
(conv) and median output 13 (code) / 129 (conv) for the 2023 traces [primary,
[arXiv 2311.18677v2](https://arxiv.org/html/2311.18677v2)]. Our computed 2023 medians are
1,469 / 1,020 and 13 / 129 — the conv numbers match exactly, the code prompt median is 2%
off, which is consistent with the released sample being the paper's sample. Good.

**The 2024 traces have no published summary-statistics table.** DynamoLLM's abstract and the
dataset README give none, and the repo's analysis notebook is the only reference
implementation. The table above is therefore, as far as we can tell, the only written-down
summary — recompute it rather than trusting this file.

### 1.3 BurstGPT — the only trace with session ids at multi-day scale

Repo: <https://github.com/HPMLL/BurstGPT>. Paper: [arXiv 2401.17644](https://arxiv.org/abs/2401.17644)
(v1 2024-01-31, v5 2025-05-26), **KDD '25**, DOI
[10.1145/3711896.3737413](https://dl.acm.org/doi/10.1145/3711896.3737413).
License: **CC-BY-4.0**. [primary] Source: a regional Azure OpenAI GPT service (so it is
Azure-hosted traffic, like the Azure traces, but a different service and a different
schema).

Schema of the v2.0 release, `BurstGPT_3.csv` (the only file with the new columns):
`Timestamp, Session ID, Elapsed time, Model, Request tokens, Response tokens, Total tokens,
Log Type`. [primary, verified by download]

- `Timestamp` — seconds from 00:00:00 on day 1, **1-second resolution**. That resolution is
  a real limitation: you cannot recover sub-second burst structure.
- `Session ID` — a UUID, **present only for `Conversation log` rows**. This is the single
  most valuable field in any public trace for prefix-cache modelling.
- `Elapsed time` — full request duration in **integer seconds** (submit → last token). Not
  TTFT. Integer seconds is coarse but it is the only public end-to-end latency field.
- `Model` — `ChatGPT` (GPT-3.5) or `GPT-4`.
- `Log Type` — `Conversation log` (the chat UI) or `API log`.
- **Failures are visible**: rows with `Response tokens == 0`. The `without_fails` variants
  strip them.

Files: `BurstGPT_1` 1.43 M rows and `BurstGPT_2` 3.86 M rows (121 days, no session id);
`BurstGPT_3` 5.34 M rows (110 days, **with** session id). [primary]

What we measured from `BurstGPT_3` [measured-here]:

| | API log | Conversation log |
|---|---|---|
| Requests | 5,110,404 | 233,617 |
| Mean rate | 0.54 req/s | 0.025 req/s |
| Request tokens mean / p50 / p90 / p99 / max | 457 / 326 / 794 / 3,141 / 125,591 | 953 / 517 / 2,354 / 5,630 / 71,197 |
| Response tokens mean / p50 / p90 / p99 | 68.9 / 14 / 111 / 804 | 272 / 187 / 669 / 1,067 |
| corr(request, response) | 0.183 | **0.438** |
| Input:output, ratio of means | 6.6:1 | 3.5:1 |
| **Frac `Response tokens == 0` (failed)** | **7.56 %** | **0.79 %** |
| Elapsed time mean / p50 / p90 / p99 (s) | 2.35 / 0 / 5 / 39 | 7.00 / 3 / 16 / 61 |

Split by model, conversation traffic: GPT-4 request tokens mean 1,304 vs ChatGPT 785;
GPT-4 response tokens mean 427 vs ChatGPT 198; GPT-4 elapsed p50 11 s vs ChatGPT 2 s.
[measured-here] Bigger model, longer prompts, longer answers, 5× the latency — worth
carrying into a multi-model fleet scenario.

Note the API-log request-token max of 125,591 — unlike the Azure traces, BurstGPT is **not**
context-truncated, so it does contain a genuine long-tail.

### 1.4 Mooncake (Moonshot AI / Kimi) — the only trace with prefix structure

Repo: <https://github.com/kvcache-ai/Mooncake>, traces under
[`FAST25-release/`](https://github.com/kvcache-ai/Mooncake/tree/main/FAST25-release).
Papers: [arXiv 2407.00079](https://arxiv.org/abs/2407.00079) (v1 2024-06-24) and the
**FAST '25** version, plus
[ACM TOS](https://dl.acm.org/doi/10.1145/3773772). **License: not stated** in the release
directory — a real problem if you intend to redistribute derived data. [primary]

Format, one JSON object per line:
`{"timestamp": ms_relative, "input_length": n, "output_length": n, "hash_ids": [...]}`.
**Block size is 512 tokens**, and identical `hash_ids` mean reusable prefix KV. The README's
own example: two requests sharing the first 12 hash ids share `12 × 512 = 6144` tokens of
prefix. [primary] `conversation_trace.jsonl` and `toolagent_trace.jsonl` are each sampled
from **one hour of online request data**; `synthetic_trace.jsonl` is built from public
datasets with Poisson arrivals. [primary]

What we measured [measured-here]:

| | conversation | toolagent | synthetic | arxiv-trace (historical) |
|---|---|---|---|---|
| Requests | 12,031 | 23,608 | 3,993 | 23,608 |
| Span | 58.9 min | 58.9 min | 17.0 min | 60.0 min |
| Mean rate | 3.40 req/s | 6.67 req/s | 3.91 req/s | 6.56 req/s |
| Input mean / p50 / p90 / p99 / max | 12,035 / 6,909 / 27,367 / 85,401 / 126,195 | 8,596 / 6,346 / 16,810 / 61,671 / 126,195 | 15,325 / 11,587 / 38,615 / 66,458 / 191,378 | 8,590 / 6,345 / 16,794 / 61,623 / 125,546 |
| Input CV | 1.31 | 1.28 | 1.20 | 1.28 |
| Output mean / p50 / p90 / p99 | 343 / 350 / 597 / 1,120 | 182 / **30** / 507 / 898 | 149 / 69 / 389 / 769 | 182 / 30 / 507 / 898 |
| Output CV | 0.73 | **1.33** | 1.19 | 1.33 |
| Input:output, ratio of means | **35:1** | **47:1** | 103:1 | 47:1 |
| Interarrival CV | 3.03 | 4.36 | 1.01 | 4.36 |
| Distinct prefix root blocks | **1** | **4** | many | 4 |

The README's own averages (12,035 / 343, 8,596 / 182, 15,325 / 149) match ours exactly, so
the file we analysed is the file they describe. The `arxiv-trace` file is the same workload
as `toolagent` with slightly different timestamp scaling — do not treat them as two
independent samples.

**The single most useful structural fact in any public trace:** the conversation trace has
exactly **one** distinct first-prefix-block hash across all 12,031 requests, and the
toolagent trace has **four** (covering 10,938 / 9,203 / 3,449 / 18 requests).
[measured-here] Every request begins with the same ≥512-token block. That is a **universal
shared system prompt**, and it means a real workload has a global prefix root, not just
per-session chains. A generator that only models per-session history growth will miss it.

### 1.5 Chat corpora: LMSYS-Chat-1M, WildChat, ShareGPT — lengths, not traces

These are **conversation text corpora**, not serving traces. Their value is length
distributions and turn counts, and their limit is that they have no server-side arrival
clock and no notion of a fleet.

| | LMSYS-Chat-1M | WildChat-1M | ShareGPT V3 cleaned |
|---|---|---|---|
| Paper | [arXiv 2309.11998](https://arxiv.org/abs/2309.11998) (ICLR 2024) | [arXiv 2405.01470](https://arxiv.org/abs/2405.01470) (ICLR 2024) | none |
| Conversations | 1,000,000 | ~1,039,785 | ~53,000 |
| Users | 210,479 unique IPs | consented opt-in users | scraped public share links |
| Models | 25 | ChatGPT / GPT-4 | ChatGPT |
| **Avg turns / conversation** | **2.0** | **~2.52** | not published |
| **Avg tokens / user prompt** | **69.5** (Llama-2 tokenizer) | not published in this form | ~202 as sampled by vLLM's bench |
| **Avg tokens / response** | **214.5** (Llama-2 tokenizer) | not published in this form | ~179 as sampled by vLLM's bench |
| Timestamps | **no** | **yes** (`timestamp` per conversation) | **no** |
| License | LMSYS-Chat-1M Dataset License Agreement, **gated** on HF | **ODC-BY** (was AI2 ImpACT) | commonly redistributed as Apache-2.0; **provenance is murky** |

[primary for LMSYS stats: [HF dataset card](https://huggingface.co/datasets/lmsys/lmsys-chat-1m)
and the paper. primary for WildChat turn count and license: HF card + paper. The ShareGPT
202/179 figures are from [vLLM's benchmark descriptions](https://github.com/vllm-project/vllm/blob/main/.buildkite/performance-benchmarks/performance-benchmarks-descriptions.md),
which sample 500 prompts with a fixed seed — they describe **vLLM's sample**, not the
corpus.]

**A warning about ShareGPT.** It is the default benchmark dataset for vLLM, SGLang, and most
papers, which means most published "realistic workload" numbers in this field are actually
ShareGPT numbers. ShareGPT is a scrape of voluntarily-shared ChatGPT links from early 2023,
with ~200-token prompts and ~180-token outputs, no timestamps, and unclear licensing. Every
number in this document that comes from a ShareGPT benchmark should be read as *"a
short-prompt chat workload from 2023,"* which is roughly the opposite of a 2026 agentic
workload (compare 8,596:182 for Mooncake toolagent). If a paper's headline result is on
ShareGPT, assume it does not transfer to long-context.

### 1.6 TraceLab — agentic sessions with cache accounting

Paper: [arXiv 2606.30560](https://arxiv.org/abs/2606.30560), "TraceLab: Characterizing
Coding Agent Workloads for LLM Serving", Zhu, Jacob, Ma, Pan, Wang, Krishnamurthy, Kasikci
(University of Washington), submitted **2026-06-29**. Repo:
<https://github.com/uw-syfi/TraceLab>. **Code Apache-2.0, dataset CC BY 4.0.** [primary,
verified 2026-09-06]

Released as GitHub release assets (`syfi_coding_trace.jsonl.gz`,
`syfi_coding_trace.duckdb`, tag `v0.0.1`): **357,161 LLM rounds**, 432,510 tool records,
**43 pseudonymous users**, `claude=140,338` / `codex=216,823` rounds, ~4,300 sessions,
Sep 2025 – Jun 2026, 23+ model versions. [primary]

We inspected the JSONL schema directly. Per round it carries:
`provider, project, session_id, round_index, round_id, model, input_tokens_total,
prefix_tokens, newly_append_tokens, claude_uncached_input_tokens,
claude_cache_creation_input_tokens, claude_cache_read_input_tokens, output_tokens,
reasoning_output_tokens`, a `timing_events` list with absolute ISO-8601 ms timestamps and
event types (`user_message`, …), and a `tools` list with per-tool
`tool_name, emitted_at, result_at, input_chars, result_chars, tool_wall_latency_ms,
is_error`. [measured-here]

This is the richest public record of an agentic workload that exists, and critically it
gives you **the prefix/append token split and the provider's own cache-read accounting per
request** — the thing every other trace lacks.

**Its limit is the arrival process.** 43 developers over 9 months is not a fleet. You can
replay session *structure* faithfully and you must **synthesise** the fleet arrival process
by superposing many sampled sessions at a chosen concurrency. Do not read a fleet-level CV
off TraceLab.

Reported workload statistics [primary, paper]:

| Quantity | Claude Code | Codex |
|---|---|---|
| Prefix tokens per step, median | **126 k** | 115.6 k |
| Prefix tokens per step, p99 | 918 k | — |
| Append tokens per step, median | 857 | 886 |
| Append tokens per step, p99 | 232 k | — |
| Output tokens, median / p90 / p99 | **252** / 1,671 / 6,571 | 184 / 939 / 3,508 |
| Input:output ratio | **~100:1** | ~100:1 |
| Requests per session, mean / p99 | **9.2** / 137 | — |
| LLM calls per request, mean | 8.8 | — |
| Tool calls per request, mean | 10.8 | — |
| Tool calls per step, mean | 1.2 | — |
| Session duration, median / p90 / p99 | 5.1 min / 5.9 h / 206.5 h | — |
| Per-request response time, median / p90 | 38.3 s / 6.4 min | — |
| Tool execution share of response time | **59.8 %** | — |
| Decode speed, median | 46.8 tok/s | 33.9 tok/s |
| Tool latency, median / p90 | 0.3 s / 13.6 s | — |
| Global prefix cache hit rate | **95.7 %** | 95.7 % |

The tool-latency tail matters for a simulator: *"calls longer than 1 min are only 4 percent
of all calls but account for 85% of total tool-call time."* [primary] An agentic session is
mostly *not* holding the GPU — it is holding session state while a tool runs.

### 1.7 Newer characterizations that are papers, not replayable traces

| Source | What it is | Scale | Trace released? |
|---|---|---|---|
| **ServeGen**, [arXiv 2505.09999](https://arxiv.org/abs/2505.09999) | Alibaba Bailian workload characterization **+ a generator** at <https://github.com/alibaba/ServeGen> | 3.54 B requests, 12 models, Jan–Apr 2025 | **No** — "parameterized and sanitized data" only. Explicitly declines prefix-cache characterization: *"characterizing prefix caching requires access to the content of requests which we currently opt out of due to confidentiality obligations."* [primary] |
| **A Year in LLM Serving**, [arXiv 2608.13573](https://arxiv.org/abs/2608.13573) (U. Chicago / Harvard / Chutes, 2026-07-03) | 12-month serverless-platform trace, **6.12 B requests**, 9,174 models, 314,970 users, Apr 2025 – Apr 2026, with per-request `cached_tokens` | 6.12 B | *"Trace release has been approved and will be added."* **We have not verified an available download.** [primary for the statement] |
| **FineServe**, [arXiv 2607.19349](https://arxiv.org/abs/2607.19349) (Tianjin Univ. + PPIO Cloud, 2026-04-17) | Multi-model marketplace dataset, 1.48 B requests, 57 models, 4 months, at <https://github.com/hihiztc1/FineServe> | 1.48 B | Claimed; license not stated; **not verified by us** |
| **From LLM Inference to Agentic Workloads**, [arXiv 2608.15127](https://arxiv.org/abs/2608.15127) (2026-08-15) | `AgentSysBench` — 10 agentic apps + instrumentation, plus production traces from 3 apps | — | Benchmark suite; trace release not stated |
| **Aliyun / Tongyi prefix-cache trace study**, [arXiv 2506.02634](https://arxiv.org/abs/2506.02634) (USENIX ATC '25) | Two production traces (to-C and to-B) analysed for KV reuse | — | Analysis only |
| **MLCommons Agentic Inference for MLPerf** ([announcement](https://mlcommons.org/2026/07/agentic-inference-for-mlperf-inference/), Jul 2026) | Multi-turn, growing-context, closed-loop agent benchmark | — | A benchmark, not a trace |

### 1.8 Recommendation: which trace for which question

| Question you are asking the simulator | Use |
|---|---|
| Does my router survive real burstiness at multi-day scale? | **Azure 2024** conv + code, replayed at scale. 168 h, 44 M requests, µs timestamps. |
| How does prefix-cache-aware routing compare to least-loaded? | **Mooncake toolagent + conversation.** It is the only trace with block-level prefix identity. Only 1 h, so loop it or superpose. |
| How do sessions and think time affect cache residency? | **BurstGPT_3 Conversation log** (session ids, 110 days) for chat; **TraceLab** for agentic. |
| What does agentic traffic do to prefill:decode ratio and KV residency? | **TraceLab** for structure, **Mooncake toolagent** for fleet-level shape. |
| Long-context / 100k+ prompts | **Mooncake** (max 126 k) or **BurstGPT API log** (max 126 k). **Not Azure** (8 k cap). |
| Multi-model fleet, heterogeneous models | **BurstGPT_3** (2 models) or FineServe / Chutes if you can get them. |
| Multimodal | **Azure LMM 2025**. |

A practical composite: take **arrival timestamps from Azure 2024 conv**, **lengths and
prefix-block structure from Mooncake toolagent**, and **session/turn structure from
BurstGPT_3 conv or TraceLab**, and be explicit in the config that you have stitched three
sources. That is more honest than pretending one trace covers everything.

---

## 2. Arrival process

### 2.1 It is not Poisson, and the answer depends on the timescale

This is the part most easily got wrong, because "CV of interarrival time" is not a single
number — it depends entirely on the window you compute it over. Over a 110-day trace the CV
is dominated by day/night idleness, not by burstiness. So we computed it three ways.

**Method 1: CV of interarrival times inside detrended 5-minute windows** (the methodology
ServeGen uses — *"CV in 5-minute windows"* [primary]).

| Trace | n windows | CV p10 | **CV p50** | CV mean | CV p90 | Frac of windows with CV > 1 |
|---|---|---|---|---|---|---|
| Azure 2024 **code** | 2,016 | 1.05 | **1.15** | 1.31 | 1.84 | **99.5 %** |
| Azure 2024 **conv** | 2,016 | 1.33 | **1.64** | 1.63 | 1.92 | **100 %** |
| BurstGPT_3 **API log** | 11,683 | 0.67 | **1.50** | 2.54 | 5.22 | 72.5 % |
| BurstGPT_3 **Conversation log** | 1,026 | 0.77 | **0.92** | 0.94 | 1.12 | 29.7 % |
| Mooncake conversation (whole hour) | — | — | **3.03** | — | — | — |
| Mooncake toolagent (whole hour) | — | — | **4.36** | — | — | — |

[all measured-here, 2026-09-06]

**Method 2: index of dispersion for counts (IDC = Var/Mean of counts in bins of width T)**
inside a single fixed hour, which removes the diurnal trend entirely. For a Poisson process
IDC = 1 at every T. Growth of IDC with T is the signature of correlated bursts /
self-similarity, and it is exactly what a simulator's arrival model has to reproduce.

| Window | rate | T=0.1 s | T=0.5 s | T=1 s | T=5 s | T=10 s | T=60 s |
|---|---|---|---|---|---|---|---|
| Azure 2024 code, day 14 18:00 UTC (peak) | 82.8/s | 2.49 | 1.46 | 1.89 | 4.52 | 6.97 | **19.86** |
| Azure 2024 code, day 14 08:00 UTC (trough) | 7.3/s | 1.09 | 1.15 | 1.42 | 2.80 | 3.97 | **7.34** |
| Azure 2024 conv, day 16 14:00 UTC (peak) | 69.2/s | 3.14 | 1.42 | 1.58 | 3.36 | 5.53 | **17.28** |
| Azure 2024 conv, day 16 22:00 UTC (trough) | 35.2/s | 1.79 | 1.16 | 1.22 | 2.07 | 3.29 | **7.02** |

[measured-here, 2026-09-06]

**Read this table, it is the load-bearing result of section 2.** IDC ≈ 1–3 at 0.1–1 s but
**7–20 at 60 s**. Two consequences:

1. **At sub-second scale the process is close to Poisson.** Within any given second, a
   Poisson approximation is defensible. That is why a naive Poisson generator produces
   plausible-looking latency histograms and lulls you into thinking it is fine.
2. **At the 10–60 s scale that matters for queueing, KV pressure, and autoscaling, the
   process is 7–20× more variable than Poisson.** A Poisson generator will understate
   sustained overload episodes by roughly an order of magnitude in variance. Every
   conclusion about queue depth, preemption rate, and autoscaler behaviour is affected.

The right model is therefore **not** "Poisson with a higher CV" — that fixes the wrong
timescale. It is a **rate process with its own dynamics**: a Markov-modulated Poisson
process (MMPP), a doubly-stochastic Cox process, or the pragmatic version — a diurnal mean
rate multiplied by a slowly-mean-reverting random rate multiplier, with Poisson arrivals
conditional on the instantaneous rate. See §10 for concrete parameters.

Supporting evidence for burst correlation: per-minute lag-1 autocorrelation of request
counts is **0.991** (Azure 2024 code) and **0.966** (Azure 2024 conv) over the full week
[measured-here] — though most of that is the diurnal trend. The trend-free version is the
IDC growth above. Independently, "A Year in LLM Serving" reports *"Most models lie above
CV = 1… Many do show positive autocorrelation. This indicates that high-load periods tend to
follow high-load periods."* [primary, arXiv 2608.13573]

### 2.2 What the literature says about the distributional family

| Claim | Source | Date |
|---|---|---|
| *"not a single stochastic process that best describes realistic arrivals in every case"* — **Gamma** best for their large-model class, **Weibull** for mid, **Exponential** "not necessarily inferior" for small; reasoning models *"roughly modeled by Poisson processes"* | ServeGen, [arXiv 2505.09999](https://arxiv.org/abs/2505.09999) [primary] | 2025-05 |
| Burstiness modelled with a **Gamma** distribution, shape α and rate β; smaller α ⇒ higher CV. Evaluations use **α = 0.5, β = 2** | BurstGPT, [arXiv 2401.17644](https://arxiv.org/abs/2401.17644) [primary] | 2024-01 / 2025-05 |
| Gamma shape α = 0.5 implies interarrival CV = 1/√α = **1.41** | [inferred] from the above | — |
| *"Most models lie above CV = 1, showing that their arrivals are bursty rather than evenly spaced"* | A Year in LLM Serving, [arXiv 2608.13573](https://arxiv.org/abs/2608.13573) [primary] | 2026-07 |
| Arrival CV depends on model class: MoE models *"high CV but comparatively lower MSSD"*, small dense models *"low CV but high MSSD"* | FineServe, [arXiv 2607.19349](https://arxiv.org/abs/2607.19349) [primary] | 2026-04 |

**Self-similarity / Hurst exponent for LLM arrivals: not found.** Nobody has published a
Hurst estimate or a formal long-range-dependence test for an LLM serving trace. Our IDC
table is consistent with long-range dependence but we did not fit a Hurst parameter and you
should not quote one.

### 2.3 Diurnal amplitude

Averaged by hour-of-day across all days in the trace, which is the honest way to state it:

| Trace | Peak/trough | Peak/mean | Shape |
|---|---|---|---|
| **Azure 2024 code** (UTC) | **8.96×** | 2.15× | Sharp working-hours pattern. Trough 08:00–10:00 UTC at 0.24× mean, peak 18:00–20:00 UTC at ~2.1× mean. |
| **Azure 2024 conv** (UTC) | **1.78×** | 1.32× | Remarkably flat. Trough 22:00 UTC 0.74×, peak 14:00 UTC 1.32×. Globally distributed chat traffic averages out. |
| **BurstGPT_3 API log** | 3.01× | 1.38× | Weak, with a pronounced dip at 09:00–15:00 local. |
| **BurstGPT_3 Conversation log** | **34.8×** | 2.36× | Extreme; a small regional service. |

[measured-here] Raw hourly buckets (not hour-of-day averaged) give Azure 2024 code a
**33.1×** ratio between its single busiest and single quietest hour of the week
[measured-here] — that is the number to use if you want a worst-case scaling scenario, and
8.96× if you want a typical day.

The important design point: **diurnal amplitude is a property of the service's user
geography, not of LLM serving.** A single-region coding assistant swings ~9× over a day; a
global chat service swings ~1.8×. Pick per scenario; do not adopt one number as "the" LLM
diurnal.

Corroborating qualitative statements: ServeGen sees *"evident diurnal fluctuations for the
arrival rate: the load peaks during the afternoons while dropping significantly in the early
mornings"* [primary]; BurstGPT sees *"periodic highs during working hours and lows during
night hours"* for conversation traffic, while *"the long-term pattern of API services follows
an aperiodic pattern characterized by burstiness"* [primary].

### 2.4 Burst duration

**Not directly published.** No source we found reports a burst-duration distribution for LLM
serving traffic. What we can offer as a proxy [inferred, measured-here]: the IDC growth
saturating between T = 10 s and T = 60 s implies burst correlation time on the order of
**10–60 s**, which is uncomfortably close to the primer's cold-start budget of 30–60 s
(§10.6). That coincidence is the whole autoscaling argument and it is worth stating in the
simulator's documentation. Treat any specific burst-duration number as **[GUESS]** and sweep
it (§11).

---

## 3. Prompt and output length distributions

### 3.1 By workload type

Consolidated. Where we computed the number it is tagged; otherwise the source is named.

| Workload | Input mean | Input p50 | Input p99 | Output mean | Output p50 | Output p99 | **In:out** | Source |
|---|---|---|---|---|---|---|---|---|
| **Code completion** (inline, 2024) | 2,511 | 1,930 | 7,685 | **22.7** | **8** | 271 | **111:1** | Azure 2024 code [measured-here] |
| **Chat, single-turn heavy** (2024) | 1,632 | 928 | 6,683 | 105.5 | 41 | 694 | **15.5:1** | Azure 2024 conv [measured-here] |
| **Chat UI, session-aware** (2023-24) | 953 | 517 | 5,630 | 272 | 187 | 1,067 | **3.5:1** | BurstGPT_3 conv [measured-here] |
| **API / programmatic chat** | 457 | 326 | 3,141 | 68.9 | 14 | 804 | **6.6:1** | BurstGPT_3 API [measured-here] |
| **Long-context chat** (Kimi, 2024-25) | 12,035 | 6,909 | 85,401 | 343 | 350 | 1,120 | **35:1** | Mooncake conversation [measured-here] |
| **Tool / agent** (Kimi, 2024-25) | 8,596 | 6,346 | 61,671 | 182 | **30** | 898 | **47:1** | Mooncake toolagent [measured-here] |
| **Coding agent** (Claude Code, 2025-26) | ~127 k (126 k prefix + 857 append, medians) | — | 918 k prefix p99 | — | **252** | 6,571 | **~100:1** | TraceLab [primary] |
| **Coding agent** (Codex, 2025-26) | ~116 k | — | — | — | 184 | 3,508 | ~100:1 | TraceLab [primary] |
| **Whole-fleet aggregate** | — | — | — | — | — | — | **3.6:1** | DeepSeek: 608 B input / 168 B output over 24 h [inferred from primary] |
| **Web-shared chat corpus** (2023) | ~202 (as sampled) | — | — | ~179 (as sampled) | — | — | **1.1:1** | ShareGPT as sampled by vLLM bench [primary] |
| **Arena chat corpus** (2023) | **69.5** | — | — | **214.5** | — | — | **0.32:1** | LMSYS-Chat-1M [primary] |
| Summarization benchmark | 8,088 | — | — | 229 | — | — | 35:1 | Mooncake Table 1, ArXiv-Summarization [primary] |
| Long-context QA benchmark | 19,019 | — | — | 72 | — | — | 264:1 | Mooncake Table 1, L-Eval [primary] |
| Kimi "real data" reference | 7,955 | — | — | 194 | — | — | 41:1 | Mooncake Table 1 [primary] |

### 3.2 The input:output ratio is the number that has moved most

Ordered, it spans nearly **three orders of magnitude**:

```
LMSYS arena chat     0.32 : 1     (short prompt, long answer)
ShareGPT             1.1  : 1
DeepSeek fleet       3.6  : 1     (aggregate over web + app + API, 2025-02)
BurstGPT chat UI     3.5  : 1
BurstGPT API         6.6  : 1
Azure conv 2024     15.5  : 1
Mooncake conv       35   : 1
Mooncake toolagent  47   : 1
Azure code 2024    111   : 1
TraceLab agent    ~100   : 1     (and ~131:1 for the vLLM×Mooncake Codex/SWE-bench set)
```

This single parameter decides whether the fleet is prefill-bound or decode-bound, and
therefore whether P/D disaggregation, chunked prefill, or decode batching is the thing worth
optimising. **It is the highest-leverage workload parameter in the whole simulator and it
must be a swept axis, never a constant.** The corollary: any conclusion drawn on ShareGPT
(1.1:1) transfers to agentic traffic (100:1) not at all.

Independent corroboration for the agentic end: the vLLM × Mooncake blog reports a
*"131:1 input-to-output ratio"* and *"average context growth of roughly 2,242 tokens per
turn"* on a Codex/SWE-bench-Pro set of 610 traces [primary,
<https://vllm.ai/blog/2026-05-06-mooncake-store>, 2026-05-06]. CacheWise reports agentic
traffic has *"~21× higher ratio of prefill to decode tokens compared to chatbot workloads"*
[secondary].

### 3.3 Distributional shape: lognormal, gamma, or a mixture?

We fitted both families to the full released datasets and computed a KS statistic against
the empirical CDF. Lognormal by log-moments (MLE), gamma by moments.

| Dataset / column | CV | lognormal σ | KS(lognormal) | gamma k | KS(gamma) | Better |
|---|---|---|---|---|---|---|
| Azure 2024 code, **input** | 0.850 | 1.209 | 0.101 | 1.385 | **0.040** | gamma |
| Azure 2024 code, **output** | 3.296 | 1.373 | **0.090** | 0.092 | 0.471 | lognormal, decisively |
| Azure 2024 conv, **input** | 0.938 | 1.006 | **0.114** | 1.138 | 0.133 | lognormal, marginally |
| Azure 2024 conv, **output** | 1.500 | 1.491 | **0.064** | 0.445 | 0.132 | lognormal |
| Azure 2023 code, **input** | 0.964 | 1.360 | 0.106 | 1.076 | **0.059** | gamma |
| Azure 2023 conv, **input** | 0.960 | 0.985 | 0.159 | 1.085 | **0.131** | gamma |
| Azure 2023 code, **output** | 2.147 | 0.847 | **0.133** | 0.217 | 0.476 | lognormal |
| Azure 2023 conv, **output** | 0.771 | 0.859 | **0.181** | 1.680 | 0.184 | ~tie |
| BurstGPT_3 all, **input** | 2.785 | 0.888 | **0.127** | 0.129 | 0.555 | lognormal |
| BurstGPT_3 all, **output** | 3.097 | 1.468 | **0.194** | 0.104 | 0.479 | lognormal |

[all measured-here, 2026-09-06]

Three conclusions:

1. **Output length is lognormal-shaped, not gamma.** Gamma is wrong by a wide margin (KS
   0.47–0.56 vs 0.06–0.19) because gamma-by-moments is forced to k ≪ 1 by the high CV and
   then puts far too much mass near zero. Use lognormal, or a mixture with a lognormal body.
2. **Input length is ambiguous and neither family fits well.** KS of 0.04–0.16 is a poor fit
   at these sample sizes — at n = 27 M, a KS of 0.11 is not a near-miss, it is a
   rejection by many orders of magnitude. The empirical input distribution is **multi-modal**:
   Azure conv shows visible clustering (p25 546, p50 928, p75 2,357, p90 3,830), consistent
   with a mixture of distinct application prompt templates.
3. **Therefore: sample input length from the empirical distribution, not a fitted family.**
   The Azure and Mooncake traces are small enough to ship an empirical quantile table. Use a
   parametric family only for the tail beyond the trace's support.

This agrees with what the literature reports for a much larger fleet. ServeGen: input
lengths are *"Pareto distributions mixed with Log-normal distributions"* while output lengths
are *"Exponential distributions fit remarkably well"* [primary] — note that exponential is
gamma with k = 1, which is not what we see, and the difference is probably that Alibaba's
Bailian traffic is dominated by API calls with a narrower spread. FineServe: output
distributions have *"most mass concentrated in 0–300 tokens"* while input lengths *"extend
substantially beyond prior reports"* [primary]. BurstGPT: request distributions *"adhere to a
Zipf distribution"* and ChatGPT's response tokens show *"a bimodal distribution"* [primary].

### 3.4 Coefficient of variation of service time

The number a queueing model needs is not the CV of output tokens but the CV of **service
time**, and output length dominates it. Output-token CV by workload [measured-here]:

| Workload | Output CV |
|---|---|
| Azure 2024 code | **3.30** |
| BurstGPT_3 API | 3.10 |
| BurstGPT_3 all | 3.10 |
| Azure 2023 code | 2.15 |
| Azure 2024 conv | 1.50 |
| Mooncake toolagent | 1.33 |
| Mooncake synthetic | 1.19 |
| BurstGPT_3 conv | 0.99 |
| Azure 2023 conv | 0.77 |
| Mooncake conversation | 0.73 |

Range: **0.7 to 3.3**. The primer's §6 claim that "service time varies by 100x or more" is
well supported — Azure 2024 code has p50 = 8 and max = 5,000 output tokens, a 625× spread,
on top of a 320× spread in input tokens. An M/G/1-style model with CV_S ≈ 1 will understate
queueing delay severely.

### 3.5 Input/output correlation

| Trace | corr(input, output) |
|---|---|
| Azure 2024 conv | **+0.392** |
| BurstGPT_3 conv | +0.438 |
| BurstGPT_3 API | +0.183 |
| Azure 2024 code | **0.000** |
| Azure 2023 code | +0.001 |
| Azure 2023 conv | −0.112 |

[measured-here] For chat traffic there is a real positive correlation of ~0.4 — longer
conversations get longer answers. For code completion it is exactly zero, because the output
is a fixed-size completion regardless of how much code precedes it. ServeGen reports the
input/output correlation for its language workload as *"weak"* [primary].

If your simulator samples input and output independently, you will be right for code
completion and wrong by ρ ≈ 0.4 for chat. That matters because positive correlation
concentrates work: the requests that cost the most to prefill also cost the most to decode.

---

## 4. Session and multi-turn structure

This section drives prefix-cache hit rate more than anything else, and it is the section
with the fewest public sources — exactly two traces carry session ids.

### 4.1 Turns per conversation

| Source | Mean | p50 | p75 | p90 | p99 | Max | Frac single-turn |
|---|---|---|---|---|---|---|---|
| **BurstGPT_3 Conversation log** [measured-here] | **4.18** | **2** | 4 | 9 | 31 | 436 | **35.5 %** |
| BurstGPT paper's own text [primary] | — | 2 | ≤4 for 75 % | — | — | — | *"Over 35% of conversations end with only one request"* |
| **TraceLab** (coding agent) [primary] | **9.2 requests/session** | — | — | — | 137 | — | — |
| **ServeGen** reasoning workload [primary] | **3.5** | — | — | — | — | — | 188,986 multi-turn of 1,964,415 total ⇒ **90.4 % single-turn** [inferred] |
| LMSYS-Chat-1M [primary] | **2.0** | — | — | — | — | — | — |
| WildChat-1M [primary] | **~2.52** | — | — | — | — | — | — |
| Aliyun to-B trace [secondary] | — | — | — | — | — | — | multi-turn ratio **< 0.1 %** |

Our BurstGPT_3 turn distribution in full [measured-here]:

| Turns | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|---|---|---|---|
| P(turns = k) | .355 | .204 | .119 | .076 | .051 | .037 | .028 | .021 | .018 | .013 |
| Cumulative | .355 | .558 | .678 | .754 | .805 | .842 | .871 | .891 | .909 | .922 |

That is close to a geometric/zeta mixture: a heavy point mass at 1 and a long tail to 436.
Ratios P(k+1)/P(k) are 0.57, 0.59, 0.64, 0.67, 0.73, 0.76, 0.75, 0.85, 0.73 — rising, so
**the more turns a conversation has already had, the more likely it continues.** A plain
geometric model with a fixed continuation probability underestimates long sessions, and long
sessions are exactly the ones with valuable cached prefixes.

### 4.2 Fraction of requests that are continuations

This is the number that sets the ceiling on session-driven cache reuse.

| Population | Fraction of requests with turn index ≥ 2 |
|---|---|
| **BurstGPT_3 Conversation log** | **76.1 %** [measured-here] |
| ServeGen reasoning workload | 188,986 / 1,964,415 = **9.6 %** [inferred from primary] |
| Aliyun to-B trace | **< 0.1 %** [secondary] |
| BurstGPT_3 as a whole (API + conv) | 233,617 conv rows of 5,344,021 total ⇒ conv traffic is only **4.4 %** of the trace, so fleet-wide continuations are ≈ **3.3 %** [inferred, measured-here] |

**Do not miss the tension in that table.** Inside chat-UI traffic, three quarters of requests
are continuations. But chat-UI traffic is a small minority of the fleet — 4.4 % of
BurstGPT_3, and Aliyun's business traffic is essentially all single-turn. The fleet-level
continuation fraction can be anywhere from 3 % to 76 % depending on the product mix, and it
is a **first-class scenario parameter**, not a constant.

The consequence for prefix caching is the single most counter-intuitive fact in this
document. Aliyun's to-B trace has multi-turn ratio < 0.1 % yet *single-turn requests
contribute 97 % of total cache hits* [secondary, from [arXiv 2506.02634](https://arxiv.org/abs/2506.02634)],
because the hits come from a hardcoded system prompt shared across users at high QPS. Our
Mooncake finding of a **single universal root block** (§1.4) is the same phenomenon measured
independently. **Shared system prompts, not conversation history, are the dominant source of
reuse in API traffic.** A simulator that models only session chains will get the wrong
answer about routing.

### 4.3 Think time between turns

From BurstGPT_3 Conversation log, 177,697 inter-turn gaps [measured-here]:

| Statistic | Value |
|---|---|
| Mean | 5,709 s (dominated by a few enormous gaps) |
| **p10 / p25 / p50 / p75 / p90 / p99** | **20 s / 42 s / 131 s / 533 s / 2,340 s / 78,903 s** |
| CV | 13.2 |
| Lognormal fit | µ = 5.18, σ = 1.96 ⇒ median 178 s |
| Frac < 10 s | 2.0 % |
| Frac < 60 s | **32.7 %** |
| Frac < 300 s | **66.0 %** |
| Frac < 3600 s | 92.4 % |
| Session duration (multi-turn only), p50 / p90 / p99 | 866 s / 20,371 s / 597,295 s |

**The p50 think time of 131 s is a hostile number for prefix caching.** OpenAI's `in_memory`
prompt cache is 5–10 minutes of inactivity, Anthropic's default TTL is 5 minutes, and
Anthropic measures the lifetime *from the start of the request that writes the entry, not
from the end of its response* [secondary]. So at p50 = 131 s roughly two thirds of
continuations land inside a 5-minute window (66.0 % < 300 s) and one third do not. **Cache
TTL is not a detail; it is a first-order determinant of hit rate**, and 5 minutes sits right
on the knee of this distribution.

Independent corroboration: ServeGen reports inter-turn times that *"concentrate around 100
seconds"* [primary] — very close to our 131 s median from a completely different service.
Two independent measurements agreeing on ~100–130 s is the most confident session number in
this document. "A Year in LLM Serving" reports the reuse-arrival distribution as *"roughly
half arrive within 0.1 second, around 80% within 10 seconds, and nearly all within several
minutes"* [primary] — a very different (much faster) picture, which is what you get when the
population is dominated by agentic tool loops rather than humans typing. TraceLab's agentic
sessions have median duration 5.1 min with 9.2 requests, implying inter-step gaps of
seconds, not minutes.

So there are **two distinct think-time regimes** and they need different parameters:

| Regime | Inter-turn gap p50 | Source |
|---|---|---|
| Human in a chat UI | **100–131 s** | BurstGPT_3 [measured-here], ServeGen [primary] |
| Agent tool loop | **~0.3 s tool median, seconds between steps** | TraceLab [primary], Chutes [primary] |

### 4.4 Context growth per turn

| Quantity | Value | Source |
|---|---|---|
| Mean request tokens, turn 1 | **170** | BurstGPT_3 [measured-here] |
| Mean request tokens, turn ≥ 2 | **1,199** | BurstGPT_3 [measured-here] |
| Mean per-turn growth in request tokens | **+187** | BurstGPT_3 [measured-here] |
| Per-turn growth p10 / p50 / p90 | −124 / **+125** / +642 | BurstGPT_3 [measured-here] |
| Context growth per turn, agentic | **~2,242 tokens** | vLLM × Mooncake [primary] |
| Append tokens per step, agentic median | **857** (Claude) / 886 (Codex) | TraceLab [primary] |

Note the p10 of **−124**: context sometimes *shrinks* between turns, because clients truncate
history. That is not noise, it is the mechanism behind the most actionable cache finding in
§5 — the truncation cliff.

---

## 5. Prefix cache hit rates

### 5.1 First, five different things are called "hit rate"

Mixing these is the biggest calibration hazard in this document. Before using any number,
establish which it is:

| Kind | Definition | Example |
|---|---|---|
| **Token-level achieved** | cached prompt tokens ÷ prompt tokens, under real cache capacity | vLLM / SGLang metrics; DeepSeek 56.3 % |
| **Token-level ideal** | the same, assuming infinite cache | Mooncake 0.51; Aliyun 62 % / 54 % |
| **Shareable-token fraction** | tokens appearing in ≥ 1 earlier prompt, from an offline prefix tree | Preble 85–97 % — **not a hit rate** |
| **Request/turn-level** | fraction of requests whose prefix was resident | Character.AI 95 % |
| **Benchmark-by-construction** | a property of a synthetic workload, not a measurement | "shared prefix" microbenchmarks |

Canonical definitions from the engines themselves:
- **SGLang**: *"We define the cache hit rate as number of cached prompt tokens / number of
  prompt tokens."* [primary, [arXiv 2312.07104](https://arxiv.org/abs/2312.07104)]
- **vLLM**: `vllm:prefix_cache_hits` = *"Prefix cache hits, in terms of number of cached
  tokens"*, `vllm:prefix_cache_queries` = *"in terms of number of queried tokens"* — so
  token-level. [primary, docs.vllm.ai]
- **TensorRT-LLM**: the metric is `trtllm_kv_cache_hit_rate`, with
  `trtllm_kv_cache_reused_blocks_total` / `..._missed_blocks_total`. [secondary]

### 5.2 Production-measured token-level hit rates

| Value | What / workload | Source | Date |
|---|---|---|---|
| **56.3 %** — *"Total input tokens: 608B, of which 342B tokens (56.3%) hit the on-disk KV cache."* | Whole-fleet, 24 h (web + app + API), DeepSeek V3/R1, 226.75 H800 nodes avg | [deepseek-ai/open-infra-index day 6](https://github.com/deepseek-ai/open-infra-index/blob/main/202502OpenSourceWeek/day_6_one_more_thing_deepseekV3R1_inference_system_overview.md) [primary, verified] | 2025-03-01 |
| **30 %** — *"All this with a relatively low cache hit ratio of 30%"*, alongside 2.5× input-token throughput and 3× lower P50 | Large-scale production batch pipeline, GPT-OSS | [Databricks](https://www.databricks.com/blog/accelerating-llm-inference-prompt-caching-open-source-models-databricks) [primary] | 2026-05-22 |
| **45.09 %** vs SGLang 28.70 % vs vLLM 19.10 % | Qwen3-Coder-480B FP8, 4P+1D on 5×8 GPUs, *"deployed in online business environment"* | [arXiv 2605.29639](https://arxiv.org/abs/2605.29639) Table 4 (Alibaba) [primary] | 2026-05-28 |
| **50 %** — *"50% hit rate for company G"*; and **85 % → 45 %** when sliding-window truncation is applied | Enterprise LMCache users, production traces | [arXiv 2510.09665](https://arxiv.org/abs/2510.09665) §9 [primary] | 2025-10-08 |
| **52.4 % / 74.1 %** — one month of production, LLaVA-Next-34B / Vicuna-33B; TTFT improved 1.7× | Chatbot Arena, one SGLang worker per model. *"Cache hits come from common system messages, frequently reused example images, and multi-turn chat histories."* | [arXiv 2312.07104](https://arxiv.org/abs/2312.07104) §6.2 [primary] | 2024-06 |
| **35 % → 70 %** after adopting cache-aware routing (plus TTFT −35 %, P95 −52 %) | Google's own Vertex AI serving fleet, *"validated on production traffic"* | [Google Cloud blog](https://cloud.google.com/blog/products/containers-kubernetes/how-gke-inference-gateway-improved-latency-for-vertex-ai) [primary] | 2026-02-06 |
| **75–80 %** | Snap, prefix-cache-aware routing on GKE Inference Gateway | [Google Cloud blog](https://cloud.google.com/blog/products/containers-kubernetes/gke-inference-gateway-prefix-caching-accelerates-ai-inference) [secondary] | 2026-06-09 |
| **40 % → 80 %**, TTFT −56 % | Qwen3-Coder-480B coding agent, ~25 k tokens / ~8 turns per session, after adding an L3 storage tier | [LMSYS SGLang HiCache](https://www.lmsys.org/blog/2025-09-10-sglang-hicache/) [primary] | 2025-09-10 |
| **95.7 %** token-weighted overall; **84.4 %** user-initiated steps, **97.5 %** tool-result steps | 357 k coding-agent rounds, from the providers' own cached-token fields | [TraceLab, arXiv 2606.30560](https://arxiv.org/abs/2606.30560) §7 [primary] | 2026-06-29 |
| **94.2 %** with a 131:1 in:out ratio; and 1.7 % → 92.2 % from adding a KV store | Codex / SWE-bench-Pro, 610 traces, 12 GB200 | [vLLM × Mooncake](https://vllm.ai/blog/2026-05-06-mooncake-store) [primary] | 2026-05-06 |
| **95 %** *request-level* (not token-level) | Character.AI consumer chat, > 20,000 QPS, sticky per-conversation routing | [research.character.ai](http://web.archive.org/web/2024/https://research.character.ai/optimizing-inference/) [primary] | 2024-06-20 |
| **90 %+ in coding** | Kimi K3, from the pricing framing ($0.30 cached vs $3.00 uncached input) | [Moonshot forum](https://forum.moonshot.ai/t/kimi-k3-is-here-our-most-capable-model/480) [primary] | 2026-07-22 |

### 5.3 Ceiling analyses on production traces — the numbers to calibrate against

| Value | Detail | Source | Date |
|---|---|---|---|
| **0.30 → 0.51** as LRU capacity goes 1,000 → ∞ blocks (512-tok blocks): 0.30 @1 k, 0.40 @10 k, 0.48 @30 k, 0.50 @50 k, 0.51 @100 k, 0.51 @∞. *"Increasing the cache capacity from 1,000 to 50,000 blocks boosts the cache hit ratio from 30% to 50%. Further capacity increases show minimal improvement."* | Kimi production trace. **The only published sizing curve from a real service.** | [Mooncake, arXiv 2407.00079](https://arxiv.org/abs/2407.00079) §4.2 [primary] | 2024-07 |
| *"Theoretically, **up to only 50 % of the KVCache can be reused** in our current workloads, even if we assume both the capacity of storage and the TTFT SLO are infinite. However, this reusability highly depends on the application scenario and **can be as large as 90 % for certain scenarios**, such as our chat-to-paper service"* | The most important sentence in the prefix-caching literature. Present already in v1. | [Mooncake, arXiv 2407.00079](https://arxiv.org/abs/2407.00079) §9 [primary] | 2024-06-24 |
| A local **3 M-token** cache reaches only **41–48 %** of the theoretical max; **50 M tokens** nearly reaches it, requiring pooling ≥ 20 nodes' DRAM. Global vs local: *"maximum increase of 136% in cache hit rate"*, prefill compute −48 % | Mooncake FAST '25 §5.3 | [Mooncake-FAST25.pdf](https://github.com/kvcache-ai/Mooncake/blob/main/FAST25-release/Mooncake-FAST25.pdf) [primary] | FAST '25 |
| **62 % (to-C) / 54 % (to-B)** ideal infinite-capacity hit rate. Plus: *"While the ideal hit ratio is high, it is smaller than the reported hit ratio (e.g., more than 80%) on synthetic workloads."* 10 % of blocks → 77 % of reuses. 90 % of blocks not reused after 612 s (to-C) / 0.3 s (to-B). A cache of **2× per-GPU HBM** approaches the ideal on GQA models. | Two Alibaba production traces | [arXiv 2506.02634](https://arxiv.org/abs/2506.02634), USENIX ATC '25 [primary] | 2025-06 |
| Ceiling **58.1 %**, vLLM achieves **35.8 %**. Workload shape: avg share-degree ≈ 3, avg common prefix ≈ 1,570 tokens, avg distinct prompt ≈ 30 tokens | BatchLLM, a *"typical industry workload"* at Microsoft | [arXiv 2412.03594](https://arxiv.org/abs/2412.03594), MLSys 2026 [primary] | 2024-11 / 2026-04 |
| Reuse rate by token type: **system prompt 92.3 %**, user prompt 30.8 %, response 27.8 %, tool output 23.0 %, **chain-of-thought 2.2 %**. And *"99.28 % (ShareGPT) / 99.96 % (AgentBank) of reused blocks are intra-session; cross-session reuse is below 0.01 %"* | SAECache | [arXiv 2605.18825](https://arxiv.org/abs/2605.18825) [primary] | 2026-05-12 |
| **85.4 % @ 1 min → ~94 % @ 5 min → 98.6 % @ 1 h** eviction timeout | Coding-agent traces, TraceLab §7.4 | [arXiv 2606.30560](https://arxiv.org/abs/2606.30560) [primary] | 2026-06-29 |
| Request-level cached fractions are **bimodal**: *"most requests see either little to no prefix reuse or reuse nearly their entire input prefix"*; *"99 % of reuse arrives within 15 minutes"*; *"LRU and FIFO often match or exceed the token hit ratios of more complex state-of-the-art algorithms"* | 6.12 B requests, 1 year, serverless platform | [arXiv 2608.13573](https://arxiv.org/abs/2608.13573) [primary] | 2026-07-03 |
| **0.1 % – 90.7 %** by task: ShareGPT multi-turn **90.7 %**, L-Eval QA **81.8 %**, L-Eval summarization **6.3 %**, HumanEval code completion **0.1 %** | The cleanest published demonstration that hit rate is a property of the *workload*, not the cache | [SwiftCache, arXiv 2606.16135](https://arxiv.org/abs/2606.16135) [primary] | 2026-06-15 |

### 5.4 What we measured ourselves from the Mooncake block hashes

Because Mooncake ships block hashes, hit rate is directly computable rather than quoted. All
numbers below use longest-matching-prefix semantics, 512-token blocks, arrival order, and
count **blocks**, which is the token-level definition [measured-here, 2026-09-06]:

**Unbounded cache, single replica:**

| Trace | Block hit rate | Mean per-request reuse fraction | p10 / p50 / p90 of per-request reuse | Frac fully cached | Frac zero reuse | Distinct blocks (working set) |
|---|---|---|---|---|---|---|
| conversation | **0.366** | 0.384 | 0.03 / 0.33 / 0.93 | 1.0 % | 0.0 % | 182,790 = 93.6 M tokens |
| toolagent | **0.553** | 0.630 | 0.06 / 0.75 / 0.92 | 1.1 % | 0.0 % | 183,300 = 93.9 M tokens |
| synthetic | 0.640 | 0.423 | 0.00 / 0.00 / 0.99 | 5.3 % | 55.4 % | 43,924 = 22.5 M tokens |

**This is a clean independent confirmation of Mooncake's own claim.** The paper says the
theoretical ceiling is "up to only 50%"; we compute 36.6 % and 55.3 % on the released traces
with an infinite cache. Two different measurements of the same service landing at ~0.4–0.55
is the most confident prefix-cache number in this document. **Do not seed a simulator with
90 %+ unless you are specifically modelling coding-agent traffic.**

Note also the p10 / p50 / p90 spread of per-request reuse (0.06 / 0.75 / 0.92 for toolagent)
— the bimodality that Chutes reports at 6.12 B-request scale is visible in a 1-hour Kimi
sample too. A simulator should sample a per-request reuse *fraction* from a bimodal
distribution, not apply a mean hit rate to every request.

**Hit rate versus LRU cache capacity** (this is the curve a simulator needs, and it is
directly comparable to Mooncake's Table 1):

| Cache blocks | Cache tokens | conversation | toolagent |
|---|---|---|---|
| 1,000 | 0.51 M | 0.044 | 0.340 |
| 2,000 | 1.02 M | 0.054 | 0.347 |
| 4,000 | 2.05 M | 0.086 | 0.370 |
| 8,000 | 4.10 M | 0.178 | 0.438 |
| 16,000 | 8.19 M | 0.263 | 0.497 |
| 32,000 | 16.38 M | 0.332 | 0.541 |
| 64,000 | 32.77 M | 0.359 | 0.552 |
| ∞ | — | 0.366 | 0.553 |

For orientation against the primer's §10.7 config: `kv_capacity_tokens: 1_370_000` is about
2,700 blocks, which on the conversation trace yields a hit rate of roughly **0.06**, and on
the toolagent trace roughly **0.35**. **A single replica's HBM is nowhere near enough to
realise the available reuse in long-context traffic.** Getting to 0.33 on conversation needs
16 M tokens of cache — an order of magnitude more than one H100 node's KV capacity, which is
precisely why Mooncake, LMCache, and Dynamo all build a pooled multi-tier cache. If your
simulator models only per-replica HBM prefix caching, it will conclude prefix caching barely
matters for long-context chat. That conclusion is an artefact of the cache tier, not the
workload.

**The cost of cache-blind routing** — the tension the primer's §5 and §6 describe, quantified
[measured-here]:

| Replicas | conversation: random routing | conversation: max/mean load | toolagent: random routing | toolagent: max/mean load |
|---|---|---|---|---|
| 1 | 0.366 | 1.00 | 0.553 | 1.00 |
| 2 | 0.269 | 1.00 | 0.492 | 1.01 |
| 4 | 0.188 | 1.01 | 0.439 | 1.01 |
| 8 | **0.128** | 1.04 | **0.400** | 1.02 |
| 16 | 0.092 | 1.05 | 0.374 | 1.03 |

Random routing across 8 replicas with unbounded per-replica caches costs **2.9×** of the
conversation hit rate (0.366 → 0.128) and **1.4×** of the toolagent hit rate (0.553 → 0.400).
The conversation trace suffers far more because its reuse is deep per-session chains that
scatter; the toolagent trace suffers less because much of its reuse is the shared root block,
which every replica caches anyway.

And the opposite failure, also measured: **unconstrained greedy longest-prefix routing
collapses to a single hot replica.** Because the conversation trace has exactly one root
block, greedy affinity sends 100 % of traffic to one replica at every N (max/mean load = N,
hit rate unchanged at 0.366) [measured-here]. That is not a subtlety to discover in
production — it is the load-vs-locality tradeoff, reproducible in ten lines of code, and it
is the strongest argument for building this simulator at all.

### 5.5 Vendor rules that bound achievable hit rate

No hyperscaler publishes a fleet hit rate. What they publish are the constraints, and three
of them are load-bearing for a simulator. All [secondary] unless marked.

| Vendor | Min cacheable prefix | TTL | Hit-rate statement |
|---|---|---|---|
| **Anthropic** | 512–4,096 tokens depending on model | 5 min default, refreshed on use; 1 h option. **Measured from the start of the writing request, not the end of its response** | Cost −90 %, latency −85 % for long prompts. *"90–99% cache hit rates"* cited for best commerce-agent deployments |
| **OpenAI** | 1,024 tokens | 5–10 min inactivity, ≤ 1 h; longer policies exist | Publishes a **measurement recipe**, and labels its example figures as hypothetical: *"a token cache-hit rate of around 70%. This is a hypothetical figure, not a measured deployment result."* [primary] Also: *"traffic above 15 requests per minute can lead to overflow routing"* [primary] |
| **AWS Bedrock** | 512–4,096 per checkpoint, ≤ 4 checkpoints | 5 min, resets on hit; 1 h for select models | The most honest vendor language anywhere: *"Implicit Prompt Caching is best effort. Repeating an identical prompt doesn't guarantee a cache hit, and cache-hit rates can vary."* **No hit rate published** |
| **Google Vertex / Gemini** | implicit 2,048–6,144 by model | explicit default 60 min, no max | **No hit rate published** |
| **Azure OpenAI** | 1,024, first 1,024 byte-identical | 5–10 min inactivity, ≤ 1 h | **No hit rate.** Publishes the same ~15 req/min per-machine ceiling |
| **DeepSeek** | 64-token storage unit; prefix must match from token 0 | *"typically within a few hours to days"* | *"The cache system does not guarantee 100% cache hits"*, but *"historical data shows that users save over 50% on average"* — which brackets their measured 56.3 % |

The ~15 requests-per-minute-per-machine ceiling that both OpenAI and Azure publish is worth
dwelling on. It is a direct statement that **cache locality is per-machine and traffic above
a modest per-prefix rate spills to machines without the entry**. That is the same phenomenon
as our §5.4 random-routing penalty, seen from the provider's side.

### 5.6 Three cautions

1. **The truncation cliff is the most actionable single fact here.** LMCache: **85 % → 45 %**
   when a sliding window trims context [primary]. Sliding-window context management is
   standard client practice and roughly halves achievable reuse. Model it as an explicit
   scenario, and note our §4.4 measurement that per-turn context growth has p10 = **−124**
   tokens, i.e. real clients do truncate.
2. **Preble's 85–97 % is a shareable-token ceiling under an infinite prefix cache, not a hit
   rate.** The authors say so: *"we construct a prefix tree for all the requests in the
   dataset (i.e., assuming an infinite prefix cache)."* [primary,
   [arXiv 2407.00023](https://arxiv.org/abs/2407.00023)] Preble reports no achieved hit rate.
3. **Router-attributable gains are not cache-attributable gains.** Headline ratios of 46×
   TTFT or 57× TTFT come from benchmarks pushed far past capacity. The same mechanism on
   Vertex AI's real traffic yields 35–52 %. Calibrate against the production numbers, not the
   demo numbers.

---

## 6. Engine efficiency constants

Four constants turn the primer's roofline (§10.4, §11) into a believable replica. Listed in
order of how much they matter, which is **not** the order of how much attention they usually
get.

A framing fact before the numbers: **published simulators of this exact kind report ~10 %
TPOT error and ~20 % TTFT error as state of the art.** Vidur reports *"less than 9 % error"*;
NVIDIA's AIConfigurator reports *"TPOT 7.8 % MAPE"* and *"TTFT 22.1 % MAPE"*, degrading to
*"25.49 % MAPE for system throughput"* under disaggregation. [primary via research pass]
That is the accuracy bar. Do not spend a week chasing 3 %.

### 6.1 Fixed per-step overhead — calibrate this first

The primer's §10.4 makes the load-bearing observation: measured batch-1 decode is 2–3× the
roofline and `t_fixed` is that gap. Here is what is actually published, from the atomic
constant upward.

**The atomic constant: kernel launch cost.**

| Value | What | Source | Date |
|---|---|---|---|
| **4.707 µs** (H100), 4.503 µs (H200) | Null-kernel launch floor, *"from `cudaLaunchKernel` call to GPU kernel start"*. H100 detail: p50 4.578, p5 4.260, p95 5.396 µs | TaxBreak, [arXiv 2603.12465](https://arxiv.org/abs/2603.12465), ISPASS 2026 [primary] | 2026-03-16 |
| **~2.1 µs** per kernel on a stream; **~1.3 µs** with CUDA graphs | Launch cost, H100 | [Hazy Research, "Look Ma, No Bubbles!"](https://hazyresearch.stanford.edu/blog/2025-05-27-no-bubbles) [primary] | 2025-05-27 |
| Framework residual on top of the floor, by kernel family: scan 0.32 µs, elementwise 0.36–0.56, reduce 0.55, GEMM-nvJit 1.18, **GEMM-cuBLAS 1.88 µs** | H100, Llama-3.2-3B prefill | TaxBreak [primary] | 2026-03-16 |

**Kernels per decode step**, which converts µs into ms:

| Value | What | Source |
|---|---|---|
| **7 launches per layer × 16 layers ≈ 112 per step**; a megakernel merges *"around a hundred separate kernels"* | Llama-1B, vLLM/SGLang, H100 | Hazy Research [primary] |
| **847.5 kernels per output token** | Llama-3.2-1B, batch 4 / seq 2048, PyTorch eager | TaxBreak Table II [primary] |
| 6,695 (Qwen1.5-MoE-A2.7B) and 9,305 (OLMoE-1B/7B) — *"8×"* and *"11× dense"* | same config | TaxBreak Table II [primary] |

847.5 kernels × 4.707 µs ≈ **4.0 ms of launch-path work per token** for a 1B dense model in
eager mode. [inferred] The launch path alone can exceed the memory roofline. **That is the
2–3× gap, quantified.** MoE makes it 8–11× worse.

**Engine-level measurements — the numbers to actually put in the config:**

| Value | What | Source | Date |
|---|---|---|---|
| **API server 33 % / scheduling 29 % / GPU execution 38 %** of total execution time. And: *"Llama3 8B can generate 1 token every 13 ms under light load."* | Llama-3 8B on 1×H100, vLLM ≤ 0.5.3 | [vLLM v0.6.0 blog](https://vllm.ai/blog/2024-09-05-perf-update) [primary, verified] | 2024-09-05 |
| ⇒ **~5 ms GPU, ~8 ms CPU** per step | 13 ms × 38 % | [inferred] — **this is exactly the primer's `fixed_step_overhead_s: 0.008`** | — |
| Multi-step scheduling **+28 % throughput** (Llama 70B, 4×H100); async output processing **−8.7 % TPOT**; object caching **+24 % throughput** | the fixes for the above | same [primary, verified] | 2024-09-05 |
| GPU model-execution time *"as low as ~5 ms"* for Llama-8B on H100; V1 gives **up to 1.7× throughput** vs V0, *"mainly due to the architectural improvements (reduced CPU overheads)"* | vLLM V1 | [vLLM V1 blog](https://vllm.ai/blog/2025-01-27-v1-alpha-release) [primary] | 2025-01-27 |
| Residual CPU share after V1: *"GPU execution dominates the overall latency (~90 %)"* | vLLM v0.10.2, 8×H800 | Frontier, [arXiv 2605.21312](https://arxiv.org/html/2605.21312v2) [primary via research pass] | 2026-06 |
| **3.05 ms = 20.6 % of the step** is the launch-side overhead CUDA graphs remove. Step goes 14.828 ms eager → **11.776 ms** with graphs | Qwen-2.5-7B, ctx 2048, batch 1, H100 | [arXiv 2605.30571](https://arxiv.org/html/2605.30571v1) [primary via research pass] | 2026-05-28 |
| CUDA graphs reduce decode TPOT by **32.3–46.5 %** (co-located) / **37.1–59.7 %** (P/D-disaggregated) | Qwen3-30B MoE, vLLM v0.10.2, 8×H800 | Frontier [primary via research pass] | 2026-06 |
| *"an unoptimized inference engine can spend as much as **half of its time on CPU overhead**"*; the zero-overhead scheduler *"runs one batch ahead"* for **1.1×** over v0.3 and **1.3×** over other baselines | SGLang v0.4, Llama-3.2-3B-Instruct | [LMSYS SGLang v0.4](https://www.lmsys.org/blog/2024-12-04-sglang-v0-4/) [primary, verified] | 2024-12-04 |
| *"V1's prefix caching causes **less than 1 % decrease in throughput** even when the cache hit rate is 0 %"* | the APC lookup itself | vLLM V1 blog [primary] | 2025-01-27 |

**Host- vs device-boundedness**, which tells you where the crossover is. TaxBreak's HDBI =
`T_DeviceActive / (T_DeviceActive + T_Orchestration)`; → 0 is host-bound, → 1 device-bound
[primary]:

| Config | HDBI |
|---|---|
| GPT-2, H200, seq 512, batch **1** | **0.25** (host-bound) |
| GPT-2, H200, seq 512, batch **8** | crossover |
| GPT-2, H200, seq 512, batch **16** | **0.74** (device-bound) |
| Qwen1.5-MoE / OLMoE decode, batch 1 | 0.15 / **0.10** |

**A batch-1 decode step below batch ~8 is majority host work.** And TaxBreak measured that
the H100 → H200 *CPU* difference alone gave a *"10–29 %"* orchestration reduction and up to
14 % end-to-end latency improvement, despite the H200 GPU running 9.9 % slower clocks. For
small-batch decode, **the host is part of the accelerator.**

**Recommendation:**

| `fixed_step_overhead_s` | When |
|---|---|
| **0.008** | Python-scheduler engine without graph capture (vLLM ≤ 0.5.3 class). The primer's default; now sourced. |
| **0.002** | Modern engine with CUDA graphs and an overlapped scheduler (vLLM V1, SGLang ≥ 0.4, TRT-LLM). Recommended default for 2026 hardware. |
| **0.0005 – 0.001** | Megakernel / persistent-kernel path. Aspirational. |
| Multiply by **3–10×** | MoE models, per TaxBreak's kernel counts. |

Sweep **0.5–8 ms**. Calibrate against one batch-1 measurement of your target model; it is the
cheapest measurement in this document and it anchors everything.

### 6.2 Memory bandwidth utilization during decode — and a trap

Concept and definition from Databricks
([LLM Inference Performance Engineering: Best Practices](https://www.databricks.com/blog/llm-inference-performance-engineering-best-practices),
**2023-10-12**) [primary, verified]:

> *"Model Bandwidth Utilization (MBU) is defined as (achieved memory bandwidth) / (peak memory
> bandwidth) where achieved memory bandwidth is ((total model parameter size + KV cache size)
> / TPOT)."*

Only two MBU values in that post are in the prose and therefore safe to quote:

| Value | Config | Source |
|---|---|---|
| **60 %** | batch 1, 512 input tokens, **2×H100-80GB**, TensorRT-LLM, MPT-7B | Databricks [primary] |
| **55 %** | batch 1, 512 input tokens, **4×A100-40GB**, TensorRT-LLM, MPT-7B | Databricks [primary] |

> **Do not read numbers off Databricks Figures 2 and 3.** Two independent extraction attempts
> of those charts disagreed with each other and with the post's own prose. An earlier draft of
> this document contained chart-read values; they have been removed. The post's only
> quotable *trend* statement is: *"MBU decreases as batch size increases. However, as we scale
> GPUs, the relative decrease in MBU is less significant."* [primary]

**The systematic batch-1 study.** *"Memory-Bound but Not Bandwidth-Limited: The Physical AI
Inference Gap in Batch-1 LLM Decode"*, [arXiv 2605.30571](https://arxiv.org/html/2605.30571v1),
2026-05-28, defines `R_floor = t_floor / t_obs` with `t_floor = (W + K)/B_peak`, which is
exactly MBU. bf16, SDPA, batch 1. Per-GPU summary over 12 (model, context) cells
[primary via research pass]:

| GPU | MBU range | **Median MBU** |
|---|---|---|
| **H100** | 0.190 – 0.310 | **0.260** |
| A100-80GB | 0.192 – 0.415 | 0.275 |
| L40S | 0.340 – 0.723 | 0.516 |
| L4 | 0.354 – 0.810 | **0.694** |

Selected cells (Llama-3.1-8B): H100 0.302 @ 2k ctx, 0.310 @ 4k, 0.279 @ 8k, **0.208 @ 16k**.
Headline, verbatim: the L4 reaches *"roughly 81 percent of its analytic memory floor"* while
the H100 reaches *"only 27 percent."*

**That is the primer's 2–3× batch-1 gap, measured and generalised: effective batch-1 MBU on
H100 is ~0.26, i.e. the step is ~3.8× the roofline.** And the *faster* the GPU, the *worse*
the ratio, because the fixed host and launch costs do not shrink with HBM bandwidth. A
simulator modelling a heterogeneous fleet must make MBU accelerator-dependent, not global.

**Engine vs hand-written kernel at batch 1** — how much of that is engine, not hardware:

| Value | What | Source | Date |
|---|---|---|---|
| **≤ 50 %** | vLLM / SGLang batch-1 bandwidth use, H100, Llama-1B: *"only able to use at most 50 % of available GPU bandwidth"* | Hazy Research [primary] | 2025-05-27 |
| **78 %** | the same workload with a megakernel: *"we use 78 % of memory bandwidth"* | same [primary] | 2025-05-27 |
| **~82 %** vs **68–75 %** | FlashFormer *"roughly 82 % bandwidth utilization, compared to the 68–75 % of GPTFast"*, 1×H100 SXM | [arXiv 2505.22758](https://arxiv.org/pdf/2505.22758) [primary via research pass] | 2025-05-28 |
| **62–88 %** | Single decode GEMM kernels on H100, derived from NVIDIA's published `gemm_perf.txt` latencies (fp16, m=1, N=K from 4096 to 16384) | [ai-dynamo/aiconfigurator](https://github.com/ai-dynamo/aiconfigurator) data, arithmetic ours | **[inferred]** |

**The gap between 62–88 % (single kernel) and 26–50 % (end-to-end) is per-step overhead.**
Constants 6.1 and 6.2 are two views of one phenomenon.

**Large batch**, achieved DRAM read fraction during decode, from *"Mind the Memory Gap"*
([arXiv 2503.08311](https://arxiv.org/html/2503.08311v2), CLOUD 2025, H100, vLLM, at max
batch) [primary via research pass]:

| Model | DRAM read % | Compute warps in flight % |
|---|---|---|
| OPT-1.3B | 47.98 % | 12.91 % |
| OPT-2.7B | 60.81 % | 31.14 % |
| Llama-2-7B | **70.55 %** | 9.85 % |
| Llama-2-13B | **76.75 %** | 10.27 % |

Also: *"more than 50 % of cycles remain idle due to data-fetching delays"* in attention at max
batch. So MBU **rises** toward 0.70–0.77 at large batch on a real engine, exactly opposite to
the batch-1 figures.

**The trap, and it is a real one: MBU and `t_fixed` double-count.** The primer's
`t_step = bytes/(HBM_bw × MBU) + t_fixed` uses both. If you set `MBU = 0.70` *and* calibrate
`t_fixed` against batch 1, you have attributed the same overhead twice. Pick a convention:

| Convention | MBU | `t_fixed` | Notes |
|---|---|---|---|
| **Recommended** | **0.75–0.85** — a pure bandwidth-efficiency number, anchored on the megakernel/FlashFormer 78–82 % | calibrated batch-1 residual, 2–8 ms | Clean separation. `t_fixed` carries launch, host orchestration, sampling, TP latency. Reproduces both the batch-1 26 % and the large-batch 70 % as *emergent*. |
| Alternative | batch-dependent: **0.26** at batch 1 (H100), **0.70** at large batch | 0 | MBU-as-fudge-factor. Simpler, but you must supply the batch curve, and it will not transfer across accelerators. |

The primer's §10.7 pairing (`mbu_decode: 0.70` with `fixed_step_overhead_s: 0.008`) sits
between the two. It gets the shape right; note the double-count in the config so nobody later
"fixes" one without the other.

### 6.3 Model FLOPS utilization during prefill

Better sourced than we first thought, but the shape of the curve is still only available on
TPU or by derivation.

**Measured single-point GPU values** [all primary via research pass]:

| MFU | Model | Hardware | Prefill tokens | Source |
|---|---|---|---|---|
| **63 %** (502 TF/s per GPU) | Llama-3 405B | 128×H100, CP16+TP, 500 W-capped, peak taken as 800 TF/s | 1,000,000 in 77 s | [arXiv 2411.01783](https://arxiv.org/pdf/2411.01783), MLSys '25 |
| **50–60 %**, *"over 50 % MFU"* at 128 GPUs | Llama-3 70B | ≤ 128×H100, TP8 + sequence-pipeline, adaptive chunking | seq len 1M–10M | Medha, [arXiv 2409.17264](https://arxiv.org/pdf/2409.17264) |
| **56.6 %** (560 TFLOPS/GPU) | Llama-3.1-70B + SwiftKV | 4×H100 TP4, vLLM, BF16 | 8 k in | SwiftKV, [arXiv 2410.03960](https://arxiv.org/pdf/2410.03960), EMNLP '25 |
| **48.5 %** (480 TFLOPS/GPU) | Llama-3.1-8B + SwiftKV | 1×H100, vLLM, BF16 | 8 k in | same; the % is [inferred] from 480/989 |
| **29.8–36.2 %**; best baseline cell 20.09 %; **< 16 %** on 8×A100 BF16 | Qwen3-235B-A22B (**MoE**) | 1–8 H100 FP8 | pure prefill, up to 128 k | [arXiv 2605.02960](https://arxiv.org/abs/2605.02960) |
| **~23–33 %** | DeepSeek-R1 (MoE, 37B active) | H100/H800 FP8, SGLang | batch 16384, input 4096 | [inferred] from CloudMatrix384 [arXiv 2506.12708](https://arxiv.org/pdf/2506.12708) Table 2 |
| **76 % / 43 % / 73 % / 36 %** | PaLM 540B and 62B | 32–64 TPU v4 | large batch vs batch 1 | Pope et al., [arXiv 2211.05102](https://arxiv.org/pdf/2211.05102), MLSys '23 |

**Dense models reach 50–63 % prefill MFU at long prompts on H100. MoE models reach 16–36 %.**
That factor-of-two-to-three gap is the single most important thing in this subsection and the
primer's single 0.45 does not capture it.

**Prompt-length dependence.** The only published MFU-versus-token curve is PaLM 540B on TPU
[primary via research pass], and it looks like this: 14 % at 80 tokens → 25 % at 160 → 34 % at
320 → 42 % at 1,024 → 44 % at 2,048 → 47 % at 4,096 → **48 % at 8,192** → a flat ~45 %
plateau from 20 k to 131 k → **76 % at 1 M** with a different (weight-gathered) layout. The
45–48 % plateau is a *partitioning* ceiling, not a hardware one: *"the 'jumps' in MFU show the
transition point from weight stationary 2D layout to XYZ weight gathered layout."*

A GPU analogue, **GEMM-only and therefore an upper bound**, derived from NVIDIA's published
`gemm_perf.txt` latencies (H100 SXM, fp16 vs 989.4 TFLOPS peak, N=K=8192) [inferred]:

| Tokens in the GEMM | 1 | 32 | 128 | 256 | 512 | 1024 | 2048 | 8192 |
|---|---|---|---|---|---|---|---|---|
| GEMM MFU (fp16) | 0.3 % | 9.2 % | 35.8 % | 64.7 % | **78.3 %** | 80.9 % | 81.5 % | 78.2 % |
| GEMM MFU (fp8) | — | — | 20.0 % | — | 48.2 % | 55.9 % | 56.1 % | 57.7 % |

Note fp8 tops out near 56–58 % of its (doubled) peak, so **switching to fp8 does not double
prefill throughput** — it gains roughly 1.4×. That is a directly useful modelling fact.

**Published saturation thresholds**, which is what the chunked-prefill budget actually depends
on [all primary via research pass]:

| Threshold | Config | Source |
|---|---|---|
| **~512 tokens** saturates prefill throughput | LLaMA-13B single layer, A6000; and Mistral-7B on 1×A100 | SARATHI [arXiv 2308.16369](https://arxiv.org/pdf/2308.16369); Sarathi-Serve [arXiv 2403.02310](https://arxiv.org/abs/2403.02310), OSDI '24 |
| Chunk **256** → ≤ 20 % prefill loss; chunk **512** → ≤ 10 %; chunk **64** → ~5× overhead | LLaMA-13B, A6000 | SARATHI |
| Execution time *"largely stagnant in the 128–512 tokens range"*, compute-bound above | LLaMA2-70B, 4×A100, TP2/TP4 | Sarathi-Serve |
| Token budgets used in practice: **2048** relaxed / **512** strict; **1536** for LLaMA2-70B relaxed | A100 | Sarathi-Serve |
| **~512 tokens** fully engages an A100 for a 13B model | 13B, 1×A100 80GB | DistServe [arXiv 2401.09670](https://arxiv.org/pdf/2401.09670) |
| Prefill throughput **decreases** above **2048** prompt tokens per batch | Llama-70B / BLOOM-176B, 8×H100 TP8, vLLM | Splitwise [arXiv 2311.18677](https://arxiv.org/pdf/2311.18677) |
| Saturation point of a 4096×4096 linear layer shifts from **~2048 tokens on A100** to **~8192 on H100** | — | [arXiv 2511.04791](https://arxiv.org/pdf/2511.04791) |

That last row matters for the primer's `max_batched_tokens_per_step: 8192`: on H100 that is
roughly the saturation point, so it is a well-chosen budget; on A100 it is 4× past it and the
budget could be 2048 with no throughput loss and much better ITL.

**Recommendation:** `mfu_prefill` of **0.50** for a dense 70B at ≥ 2 k prompt tokens on H100
(range 0.40–0.63), **0.25** for MoE (range 0.16–0.36), and scale down sharply below 512
tokens per chunk using the GEMM curve above as the shape. Practical peak to divide by: NanoFlow
measured *"the profiled peak Compute is 280 TFLOPS for FP16"* on 8×A100 versus 312 datasheet
[primary via research pass], so **~90 % of datasheet is the realistic GEMM ceiling** and MFU
against datasheet peak carries a built-in 10 % haircut.

**Do not cite** the widely repeated *"prefill achieves 30–50 % MFU"* or *"Llama 70B hits 92 %
compute utilization during prefill"* — both trace only to secondary blogs with no methodology,
and the 92 % is almost certainly SM occupancy, not MFU.

### 6.4 Tensor-parallel all-reduce cost per layer

**NVIDIA publishes measured tables for this** — the `aiconfigurator` repository ships profiled
latency data (paper: [arXiv 2601.06288](https://arxiv.org/abs/2601.06288), NVIDIA, 2026-01-09,
*"~30 GPU-hours per platform-framework pair"*). H100 SXM 80GB HBM3, float16, **per all-reduce,
microseconds** [primary via research pass, ms→µs conversion ours]:

| TP | 4 KB | 16 KB | 64 KB | 256 KB |
|---|---|---|---|---|
| 2, custom (AUTO) | 8.27 | 8.60 | 8.70 | 10.74 |
| 2, NCCL | 10.61 | 10.94 | 10.96 | 11.75 |
| 4, custom | 8.21 | 8.69 | 9.20 | 12.26 |
| 4, NCCL | 13.31 | 14.90 | 15.12 | 15.57 |
| **8, custom** | **8.96** | **10.25** | **11.02** | 16.71 |
| 8, NCCL | 19.42 | 20.84 | 23.02 | 23.60 |

**The structural fact a simulator needs: latency is essentially flat below 64 KB.**
Small-message all-reduce is a **fixed cost per collective**, not a bandwidth term — exactly as
the primer's §10.4 formula assumes. And TP decode messages *are* small: NVRAR reports typical
TP decode all-reduce payloads of *"128 KB to 1 MB"*, with a 70B model at batch 8 and hidden
8192 producing **128 KB** messages. [primary via research pass]

**Best academic measurement**, SiFAR ([arXiv 2607.08973](https://arxiv.org/html/2607.08973v1),
MICRO 2026, 8×H200, CUDA 12.9), 8 KB payload, µs [primary via research pass]:
TP2 one-shot **3.36**, TP4 two-shot **4.38**, TP8 one-shot **4.01** / two-shot 5.11; SiFAR
itself 2.36–2.44 at all TP degrees. Barriers alone are *"32–50 % of one-shot and 49–62 % of
two-shot latency for small payloads."*

**All-reduce as a fraction of step time** — the number to sanity-check against:

| Fraction | Config | Source |
|---|---|---|
| **0 % → 30 % of the decode step going TP1 → TP8**, with TPOT falling 2.73 ms → 1.43 ms (a 43 % throughput gain) | Llama-3.1-8B, 8×H200, megakernel | SiFAR [primary via research pass] |
| up to **23 % of end-to-end** inference latency, *"can be over 20 %"* even with NVLink/NV-SHARP; RMSNorm a further 5–9 % | Llama-3.3-70B, Qwen2.5-72B, Mixtral-8x22B, 8×H100 DGX, TP4/TP8, chunked prefill 2048 | TokenWeave [arXiv 2505.11329](https://arxiv.org/html/2505.11329v1) |
| up to **65 % of prefill latency on 4×L40**; *"a notable 20 %"* on A100 | LLaMA-3-70B, TP4–TP8 | Flash Communication [arXiv 2412.04964](https://arxiv.org/html/2412.04964v1) |
| Communication 47.92 ms of a ~225 ms iteration ⇒ **~21 %** | Llama2-70B, 8×A100, 2 k dense mixed batch | NanoFlow Table 2 [primary via research pass]; the % is [inferred] |

**Collectives per step:** *"an 80-layer LLaMA-3-70B carries out 160 all-reduce operations at
each forward pass"* — confirming **2 per layer**, exactly as the primer says. [primary via
research pass, Flash Communication]

**Recommendation:**

```
tp_allreduce_us_per_collective:  9   # TP8, custom kernel, <64 KB. Range 4 - 23.
collectives_per_layer:           2
```

At 80 layers × 2 × 9 µs = **1.44 ms per decode step** for a 70B at TP8. Against the primer's
7.5 ms roofline that is ~19 % — consistent with SiFAR's 30 % and TokenWeave's ≤ 23 %. Use
**~20 µs** if you are modelling plain NCCL rather than a fused custom kernel; that is 3.2 ms
and 42 % of the roofline, which is why every engine ships a custom small-message all-reduce.
Model it as **latency-dominated and scaling with layer count**, never as a bandwidth term.

Absolute floor, for reference: *"the SoL latency of an AllReduce is computed to be 1.404 µs"*
on 2 GB200 GPUs, and NCCL ring at 128 B on 4 GB200 improves from *"11.0 µs … to 2.37 µs"* with
tuning [primary via research pass, [arXiv 2607.16100](https://arxiv.org/html/2607.16100v1)].

**Not found:** vLLM has never published a custom-all-reduce latency table against NCCL, and
TensorRT-LLM's MultiShot blog puts its µs values in a figure only (its text gives ratios: Ring
takes *"2N−2 communication steps"*, MultiShot *"2 communication steps (regardless of number of
GPUs)"*, *"AllReduce latency is reduced by up to 3×"*).

### 6.5 Absolute anchors to validate against

**Batch-1 (concurrency-1) decode.** NVIDIA NIM's benchmarking docs are the only
clearly-specified public source. NVIDIA DGX H100, H100 80GB HBM3, driver 570.124.06, AIPerf,
page last updated **2026-04-01**. Throughput is **output tokens/s for the whole TP group**,
which at concurrency 1 equals per-user speed [primary, verified for the starred rows]:

| Model | GPUs / TP | Precision | ISL/OSL | TTFT ms | **ITL ms** | out tok/s |
|---|---|---|---|---|---|---|
| Llama-3.3-70B | 8×H100 TP8 | bf16 | 1000/1000 | 57.63 | **10.25** | **97.06** ★ |
| Llama-3.3-70B | 4×H100 TP4 | fp8 | 1000/1000 | 51.01 | 13.99 | 71.26 ★ |
| Llama-3.3-70B | 4×H100 TP4 | bf16 | 1000/1000 | 78.59 | 15.49 | 64.30 |
| Llama-3.3-70B | 2×H100 TP2 | fp8 | 1000/1000 | 82.46 | 18.67 | 53.37 |
| Llama-3.3-70B | 2×H100 TP2 | fp8 | 20000/2000 | 1833.01 | 19.58 | 48.80 |
| Llama-3.1-8B | 1×H100 TP1 | fp8 | 1000/1000 | 19.03 | **4.53** | **220.10** ★ |
| Llama-3.1-8B | 1×H100 TP1 | bf16 | 1000/1000 | 27.36 | 6.52 | 152.84 |
| Llama-3.1-8B | 1×H100 TP1 | fp8 | 20000/2000 | 403.20 | 5.06 | 189.96 |

**This refines the primer's §10.4 batch-1 estimate.** The primer predicts ~15–25 ms per token
and ~65 tok/s for a 70B on 8×H100 against a 7.5 ms roofline (a 2–3× gap). The measured NIM
figure for Llama-3.3-70B bf16 TP8 is **10.25 ms and 97 tok/s** — a gap of only **~1.4×**. So
on a well-tuned engine with CUDA graphs and a custom all-reduce, `t_fixed` is about **2.7 ms**,
not 8 ms. The primer's 2–3× is right for a 2024-era Python-scheduler engine and pessimistic by
about 2× for a 2026 one. **This is the single most useful correction in section 6:** it moves
the whole low-batch region of the latency-versus-load curve.

Also visible in that table and worth encoding: **TP scaling at batch 1 is strongly
sublinear** — TP2 53.4 → TP4 71.3 → TP8 97.1 tok/s, i.e. 4× the GPUs buys 1.8× the speed.
[inferred from the rows above] That is §6.4's all-reduce cost showing up end to end.

**Max-throughput anchors.** TensorRT-LLM's `perf-overview.md`, "Total Output Throughput
(tokens/sec)" per system under infinite request rate [primary via research pass]:

| Model | GPUs | ISL/OSL | tok/s |
|---|---|---|---|
| Llama-3.1-70B FP8 | 8×H100 TP8 | 128/2048 | 17,464 |
| Llama-3.1-70B FP8 | 8×H100 TP8 | 1000/1000 | 11,082 |
| Llama-3.1-70B FP8 | 8×H100 TP8 | 2048/2048 | 8,773 |
| Llama-3.1-70B FP8 | 8×H100 TP8 | 20000/2000 | 1,569 |
| Llama-3.1-8B FP8 | 1×H100 | 1000/1000 | 15,270 |
| Llama-3.1-8B FP8 | 1×H100 | 20000/2000 | 1,341 |
| **Llama-3 70B, FP8 vs FP16** | 8×H100 TP8 | 1000/1000 | **11,155 vs 5,617** |
| **Llama-3 8B, FP8 vs FP16** | 1×H100 | 1000/1000 | **13,372 vs 7,041** |

The primer's §10.7 sanity check expects ~8–10 k output tokens/s per replica for a 70B at 4 k
context. TRT-LLM measures 8,773 at 2048/2048 and 11,082 at 1000/1000 on 8×H100 FP8.
**The primer's expectation is confirmed.**

The FP8-vs-FP16 pairs are worth keeping: **FP8 is ~2× FP16 at max throughput**, considerably
better than the ~1.4× that §6.3's GEMM curve suggests for prefill alone — because at max
throughput the win is mostly in KV capacity and decode bandwidth, not prefill FLOPs.

**Production anchor.** DeepSeek V3/R1, 24 h, 226.75 average 8-GPU H800 nodes: *"~73.7 k
tokens/s input (including cache hits) during prefilling or ~14.8 k tokens/s output during
decoding"* per node; average output speed **20–22 tokens/s** per request; average KV length per
output token **4,989 tokens** [primary, verified]. This is the only fleet-scale number with a
stated node count and both phases. 20–22 tok/s per request at ~5 k context for a 671B MoE sits
squarely in the primer's §10.4 realistic band.

**Client-observed anchor.** TraceLab measures median decode **46.8 tok/s** (Claude) and
**33.9 tok/s** (Codex) across 357 k real agentic rounds, with **CV > 50 %** [primary]. That CV
is queueing and batch composition, and reproducing it is a good validation target.

**Benchmark pitfalls, all of which will bite you:**

- TRT-LLM `perf-overview` and NIM report **whole-TP-group** output tokens/s. Divide by TP for
  per-GPU.
- Both are **output-only**. Do not compare them to total-token figures.
- **MLPerf's 70B benchmark is Llama-2-70B, not Llama-3-70B**, and there is no Llama-3-70B
  MLPerf benchmark in any round. MLPerf's `llama3.1-8b` is *summarization* and is
  prefill-dominated, so its tok/s is not comparable to TRT-LLM's 128/2048 rows. NVIDIA
  submitted no H100 system at all in MLPerf v5.1. [primary via research pass]
- Llama-3.1-70B at TP1 on one H100 is **KV-starved** (745 tok/s at 128/2048 vs 3,191 at
  128/128) and is not a clean TP-scaling baseline.
- `perf.vllm.ai` was returning HTTP 522 as of 2026-09-06, and the vLLM v0.6.0 and V1 blog
  performance data is **figures only** — no numeric Llama-3-on-H100 tables exist in either.

---

## 7. Speculative decoding acceptance rates

The primer's §12 lists speculative decoding as ignorable in v1 but flags that *"acceptance rate
falls as batch size rises, because the bottleneck shifts."* **That conflates three distinct
effects, and getting them apart is the whole content of §7.4.** The correction matters the
moment speculative decoding enters scope.

### 7.1 Per-token acceptance rate α, by draft/target pair

α is the probability that the target accepts a given drafted token. From Leviathan, Kalman &
Matias, [arXiv 2211.17192](https://arxiv.org/abs/2211.17192) (ICML 2023), Table 3
[primary via research pass]:

| Target | Draft | α (T=0) | α (T=1) |
|---|---|---|---|
| GPT-like 97M | GPT-like 6M | **0.88** | 0.89 |
| T5-XXL 11B (EnDe) | T5-small 77M | 0.75 | 0.62 |
| T5-XXL 11B (EnDe) | T5-base 250M | 0.80 | 0.68 |
| T5-XXL 11B (EnDe) | T5-large 800M | **0.82** | 0.71 |
| T5-XXL 11B (CNN/DM) | T5-small 77M | 0.65 | 0.53 |
| T5-XXL 11B (CNN/DM) | T5-large 800M | 0.74 | 0.56 |
| LaMDA 137B | LaMDA 100M | 0.61 | 0.57 |
| LaMDA 137B | LaMDA 2B | 0.71 | 0.71 |
| LaMDA 137B | LaMDA 8B | **0.75** | 0.74 |
| any of the above | **unigram** | 0.03 – 0.13 | 0.03 – 0.08 |
| any of the above | **bigram** | 0.05 – 0.23 | 0.05 – 0.19 |

Three usable rules fall out [inferred from the table]:
- **Temperature 1 costs 0.06–0.19 of α** versus greedy, for the same pair. Sampling temperature
  is a first-order driver.
- **Non-neural n-gram drafts give α of 0.03–0.23**, an order of magnitude below any neural
  draft. Yet they still win sometimes, because their cost is ~0 (see §7.3).
- Draft size has **diminishing returns**: 77M → 800M moves α only 0.75 → 0.82.

Modern independent measurements:

| α | Config | Source | Date |
|---|---|---|---|
| **53.5 % / 54.0 %** (1 k ctx), **50.9 %** (2 k ctx) | Qwama-0.5B draft → Llama-3.1-70B BF16 TP4 | [SqueezeBits](https://blog.squeezebits.com/vllm-vs-tensorrtllm-11-speculative-decoding-37301) [primary via research pass] | 2024-12-09 |
| **76.5 %** (1 k ctx) | Llama-3.1-8B draft → Llama-3.1-70B | same | 2024-12-09 |
| **85–90 %** for the second predicted token, *"across various generation topics"*, giving *"1.8 times TPS"* | DeepSeek-V3 native multi-token prediction | [arXiv 2412.19437](https://arxiv.org/abs/2412.19437) §5.4.3 [primary via research pass, PDF-verified] | 2024-12 |
| **45.4 %** (2 draft tokens), **35.6 %** (3), **28.3 %** (4) | EAGLE3 head → gpt-oss-120b MoE MXFP4, H200, vLLM v0.13.0 | [Red Hat](https://developers.redhat.com/articles/2026/04/16/performance-improvements-speculative-decoding-vllm-gpt-oss) [primary via research pass] | 2026-04-16 |
| **94.8 %** at N=1 falling to **80.0 %** at N=5 | Gemma-4 MTP on GSM8K, MI300X/MI355X | vLLM AMD blog [primary via research pass] | 2026-08-23 |

**Position decay** is the mechanism behind α falling as draft length grows. EAGLE's own
per-position rates on MT-bench at T=0 are 0.74–0.79 at position 0 decaying to 0.64–0.71 at
position 4 [primary via research pass, [arXiv 2401.15077](https://arxiv.org/abs/2401.15077)
Table 2]. And the cleanest cumulative statement, from vLLM's own blog: *"At three speculative
tokens, the first, second, and third draft positions are accepted about **76 %, 56 %, and
43 %** of the time (cumulative)"* [primary via research pass, vLLM EAGLE3-on-AMD blog].

> **Do not cite an acceptance rate from Chen et al. ([arXiv 2302.01318](https://arxiv.org/abs/2302.01318)).**
> A figure of *"87 % of tokens accepted"* with a *"7-layer 1.3B draft"* circulates attributed to
> that paper. Both are fabrications — neither string exists in the PDF, the real draft is 4B
> with 8 layers, and the paper publishes acceptance rate only as an unlabelled figure panel.
> [verified against the PDF]

### 7.2 Acceptance length τ — the number a simulator actually needs

τ = tokens generated per draft-verify cycle, so it is what divides the step count. The best
apples-to-apples table in the literature is EAGLE-3's Table 1: eight methods, five datasets,
one harness, **batch size 1**, Vicuna-13B target, T=0, baseline = 1.00× [primary via research
pass, PDF-verified]:

| Method | MT-bench | HumanEval | GSM8K | Alpaca | CNN/DM | **Mean (speedup / τ)** |
|---|---|---|---|---|---|---|
| Standard spec. sampling (68M draft) | 1.93× / 2.27 | 2.23× / 2.57 | 1.77× / 2.01 | 1.76× / 2.03 | 1.93× / 2.33 | **1.92× / 2.24** |
| Prompt lookup (n-gram) | 1.58× / 1.63 | 1.85× / 1.93 | 1.68× / 1.73 | 1.16× / 1.19 | **2.42× / 2.50** | 1.74× / 1.80 |
| Lookahead | 1.65× / 1.69 | 1.71× / 1.75 | 1.81× / 1.90 | 1.46× / 1.51 | 1.46× / 1.50 | 1.62× / 1.67 |
| Medusa | 2.07× / 2.59 | 2.50× / 2.78 | 2.23× / 2.64 | 2.08× / 2.45 | 1.71× / 2.09 | 2.12× / 2.51 |
| Hydra | 2.88× / 3.65 | 3.28× / 3.87 | 2.93× / 3.66 | 2.86× / 3.53 | 2.05× / 2.81 | 2.80× / 3.50 |
| EAGLE | 3.07× / 3.98 | 3.58× / 4.39 | 3.08× / 3.97 | 3.03× / 3.95 | 2.49× / 3.52 | 3.05× / 3.96 |
| EAGLE-2 | 4.26× / 4.83 | 4.96× / 5.41 | 4.22× / 4.79 | 4.25× / 4.89 | 3.40× / 4.21 | 4.22× / 4.83 |
| **EAGLE-3** | 5.58× / 6.65 | **6.47× / 7.54** | 5.32× / 6.29 | 5.16× / 6.17 | 5.01× / 6.47 | **5.51× / 6.62** |

Same table for other targets at T=0 (mean speedup / τ): Llama-3.1-8B EAGLE-2 3.23×/4.11,
EAGLE-3 4.44×/6.23; **Llama-3.3-70B EAGLE-2 2.85×/3.78, EAGLE-3 4.12×/5.88**;
DeepSeek-R1-Distill-Llama-8B EAGLE-3 4.16×/5.84. At T=1, EAGLE-3 on Vicuna-13B drops to
4.65×/5.67. [primary via research pass]

**Task dependence is large and systematic:** τ is highest on **HumanEval (code)** and lowest on
**CNN/DM (summarization)** and **Natural Questions** for every method. EAGLE-2's τ on Vicuna-13B
ranges 5.41 (HumanEval) down to 3.74 (Natural Questions). [primary via research pass] So a
simulator's τ must be conditioned on workload class, mirroring §3.

**Production τ is much lower than research τ**, because production K is small:

| τ | Config | Source |
|---|---|---|
| 1.91 / 2.07 / 2.13 | EAGLE3 → gpt-oss-120b at 2 / 3 / 4 draft tokens | Red Hat 2026-04-16 |
| 2.80 average (Coding 3.32, Math 3.14, RAG 3.12, … Roleplay 2.01) | EAGLE3 → MiniMax-M3, SPEED-Bench | vLLM AMD blog 2026-07-13 |
| 2.18 (3-token MTP) / 2.44 (4-token) | DeepSeek V3 on H200 | [LMSYS MTP blog](https://www.lmsys.org/blog/2025-07-17-mtp/) 2025-07-17 |
| 2.75 – 2.94 "tokens per call" | Llama 3.1 8B / 3.3 70B / Llama 4 Scout / Maverick | Meta, [arXiv 2508.08192](https://arxiv.org/abs/2508.08192) |

**Context-length invariance:** MiniMax-M3's acceptance length is *"essentially flat from 1 K to
32 K context"* (2.69 → 2.65) [primary via research pass]. SqueezeBits saw a small opposite
effect on α (54.0 % at 1 k → 50.9 % at 2 k). Treat τ as **context-independent to first order**
and note the disagreement.

### 7.3 The evidence that α is a model property, not a system property

Spec-Bench's leaderboard runs the same methods on two different GPUs
([Leaderboard.md](https://github.com/hemingkx/Spec-Bench/blob/main/Leaderboard.md), updated
2025-04-22, Vicuna, greedy, FP16, batch 1) [primary via research pass]:

| Method | Mean accepted tokens, RTX 3090 | Mean accepted tokens, A100 | Overall speedup, 3090 → A100 |
|---|---|---|---|
| Lookahead | **1.64** | **1.63** | 1.13× → 1.30× |
| Recycling | **2.73** | **2.73** | 1.40× → 2.17× |
| Medusa | **2.32** | 2.39 | 1.44× → 1.80× |
| EAGLE-2 | 4.35 | 4.43 | 2.19× → 2.46× |

**Accepted-tokens-per-step is essentially identical across hardware while speedup moves by up
to 1.55×.** That is the cleanest empirical separation available of the model property from the
system property, and it is the reason §7.4's correction holds.

Note also from that table that **α and speedup do not rank the same way**. Prompt lookup has
τ = 1.80, well below Medusa's 2.51, yet beats Medusa on summarization (2.42× vs 1.71×) because
its draft is free. **Draft cost matters as much as α.** SmartSpec measured PLD's α on Mistral-7B
at *"between 0.3 and 0.4"* and it still delivered *"substantial speedup"* [primary via research
pass].

### 7.4 How batch size actually interacts — the correction

Three distinct effects, routinely all called "acceptance rate":

1. **Per-token acceptance rate α is batch-invariant.** It is a property of the
   (draft, target, task, temperature, position) tuple. No primary source measures it falling
   with batch, and MagicDec's speedup decomposition
   `T_ArS/T_SD = Ω(γ, α) · T_T / (γ·T_D + T_V(γ))` contains **no batch term in Ω** — all batch
   dependence lives in the cost terms. [primary via research pass]
2. **Speedup does fall with batch**, because decode moves from bandwidth-bound to compute-bound
   and the spare FLOPs speculation spends stop being free. EAGLE-3 states the mechanism
   verbatim: *"Speculative sampling algorithms like EAGLE-3 reduce memory accesses and lower
   latency during memory-bound decoding by leveraging redundant computational power. As batch
   sizes increase, this redundancy decreases, reducing the effectiveness of speculative
   sampling."* [primary via research pass, PDF-verified] Note the framing: **compute redundancy
   disappearing, not the target becoming less likely to accept.**
3. **A tuned scheduler deliberately shrinks K as batch grows**, which lowers τ without changing
   α. This is the honest mechanism behind papers that appear to show "acceptance rate falling
   with batch" — check their metric definition. *"Scaling Laws for Speculative Decoding"*
   ([arXiv 2505.07858](https://arxiv.org/abs/2505.07858)) defines "acceptance rate" as a token
   *count*, and its actual finding is that **optimal draft top-k shrinks with batch**.
   vLLM's Dynamic Speculative Decoding makes the policy explicit: *"When this BS*K goes beyond a
   critical BS then SD negatively impacts the decode speed (TPOT)"*, with a documented example
   ladder of **K=3 for batch 1–64, K=1 for 65–128, K=0 for 129–512**. [primary via research
   pass]

**Measured batch ladders.** EAGLE-3 Tables 3 and 5, Llama-3.1-8B, MT-bench, throughput relative
to no-speculation = 1.00× [primary via research pass, PDF-verified]:

| Batch | 2 | 4 | 8 | 16 | 24 | 32 | 48 | 56 | 64 |
|---|---|---|---|---|---|---|---|---|---|
| EAGLE, SGLang / H100, chain 3 | 1.40× | 1.38× | 1.23× | 1.02× | **0.93×** | 0.94× | 0.88× | 0.99× | 0.99× |
| EAGLE-3, SGLang / H100, chain 3 | 1.81× | 1.82× | 1.62× | 1.48× | 1.39× | 1.32× | 1.38× | 1.34× | **1.38×** |
| EAGLE, vLLM / A100, chain 2 | 1.30× | 1.25× | 1.21× | 1.10× | 1.03× | **0.93×** | 0.82× | **0.71×** | — |
| EAGLE-3, vLLM / A100, chain 2 | 1.75× | 1.68× | 1.58× | 1.49× | 1.42× | 1.36× | 1.21× | **1.01×** | — |

**Measured crossover points, where speculative decoding becomes a net loss:**

| Crossover | Config | Source |
|---|---|---|
| **batch 24** (0.93×), worsening to 0.88× | EAGLE-1, SGLang / H100, chain 3 | EAGLE-3 Table 3 |
| **batch 24–32**, degrading monotonically to **0.71× at 56** | EAGLE-1, vLLM / A100, chain 2 | EAGLE-3 Table 5 |
| just past **batch 56** (1.01×) | EAGLE-3, vLLM / A100, chain 2 | EAGLE-3 Table 5 |
| **none observed through batch 64** (still 1.38×) | EAGLE-3, SGLang / H100, chain 3 | EAGLE-3 Table 3 |
| **concurrency ~32–64** at 1 k context, dropping to **~16 at 2 k context** | Qwama-0.5B → Llama-3.1-70B, TP4 | SqueezeBits |
| **request rate 12** for K=5; **request rate 16** for K=3 | SmartSpec / TurboSpec, [arXiv 2406.14066](https://arxiv.org/abs/2406.14066) | primary via research pass |
| **1.4× slowdown** (ShareGPT) and **1.8× slowdown** (CNN/DailyMail) at high QPS | Llama3-70B, 4×H100 | [vLLM blog](https://vllm.ai/blog/2024-10-17-spec-decode) [primary, verified] |
| **no crossover through 200 concurrent requests** (+10–21 % throughput throughout) | EAGLE3 head → gpt-oss-120b MoE, H200, vLLM v0.13.0 | Red Hat 2026-04-16 |

**The crossover is not a constant.** Observed range across primary sources: **batch 16 to well
past 200**, depending on draft quality, K, context length, hardware, and framework version.
Present the 2024 and 2026 evidence together rather than picking one: vLLM's 2024 *"1.4–1.8×
slowdown at high QPS"* used a 0.5B external draft; Red Hat's 2026 *"gains persist up to 200
concurrent requests, contradicting the conventional expectation"* used a trained EAGLE3 head
two framework generations later. Both are primary and both are correct for their setup.

**Two documented sign flips.** Do not model `d(speedup)/d(batch)` as monotonically negative:

1. **Long context inverts it.** MagicDec: *"for S ≥ S_inflection, the speculative decoding
   speedup tends to increase with batch size."* Measured, Llama-2-7B-32K with TinyLlama-1.1B
   draft on 8×A100 [primary via research pass]:

   | Sequence length | batch 32 | batch 64 | batch 128 |
   |---|---|---|---|
   | 4,000 | 1.13× | 1.30× | 1.32× |
   | 8,000 | 1.27× | 1.51× | 1.60× |
   | 16,000 | 1.50× | 1.72× | — |

   The mechanism is the primer's own §10.4 point 3: past batch ~64 at 4 k context the **KV read
   term overtakes the weight read term**, and KV read grows with batch, so the step stays
   bandwidth-bound and speculation keeps paying.
2. **Model size flips it.** Meta, on their own production Llama stack: *"the Llama3.1 8B model
   exhibits greater speculative decoding speedup at large batch sizes compared to small batch
   sizes. In contrast, the speed-up for Llama4 Maverick, which has approximately 400 billion
   parameters, decreases with increasing batch size."* [primary via research pass,
   [arXiv 2508.08192](https://arxiv.org/abs/2508.08192)]

### 7.5 Recommended structure and defaults

Model four quantities separately, in this order:

```yaml
speculative_decoding:
  # (1) alpha: per-position acceptance probability. BATCH-INVARIANT.
  alpha_position_0: 0.76          # vLLM EAGLE3 measured cumulative
  alpha_decay_per_position: 0.75  # 76% -> 56% -> 43% cumulative implies ~0.75 per step
  alpha_temperature_penalty: 0.12 # subtract for T=1 vs greedy; range 0.06-0.19
  alpha_by_task:                  # multiplier on alpha; from EAGLE-2/3 task spread
    code: 1.12
    math: 1.00
    chat: 1.00
    summarization: 0.87
    rag: 0.95
  alpha_ngram_draft: 0.35         # non-neural draft; range 0.03-0.40

  # (2) K: draft length. A SCHEDULER DECISION, legitimately batch-dependent.
  k_ladder:                       # vLLM DSD's own documented example
    - [1, 64, 3]
    - [65, 128, 1]
    - [129, 512, 0]

  # (3) tau: emergent from alpha and K. Do not set directly; validate against:
  tau_expected_bs1_production: 2.8    # EAGLE3 in production, K=3
  tau_expected_bs1_research: 6.6      # EAGLE-3 paper, unconstrained K

  # (4) speedup: must be EMERGENT from the cost model, never a parameter.
  draft_cost_fraction_of_target_step: 0.10   # [GUESS]; range 0.05-0.25
```

**The speedup must be emergent.** In the primer's §11 formulation, a speculative step reads the
same weights once but does `(K+1)×` the attention and MLP compute, so:

```
t_spec_step = max( bytes_read / eff_bw ,  (K+1) * compute_time ) + t_fixed + t_draft
```

Let `max()` produce the regime change. If your simulator has to be *told* that speculative
decoding stops helping at batch 32, its decode cost model is not capturing the compute term —
and it will also miss MagicDec's long-context inversion, which falls out of the same `max()`
once KV read is in `bytes_read`.

**Not found, and it is the important gap:** **no source anywhere publishes acceptance rate
broken down by concurrency level.** Red Hat has both the concurrency sweep and the acceptance
rates but reports the latter only aggregated. So the batch-invariance of α rests on MagicDec's
decomposition plus Spec-Bench's hardware-invariance evidence, not on a direct measurement.
State it that way.

**Also not found:** acceptance-rate numbers for Draft & Verify
([arXiv 2309.08168](https://arxiv.org/abs/2309.08168)), LayerSkip
([arXiv 2404.16710](https://arxiv.org/abs/2404.16710)), or SpecInfer
([arXiv 2305.09781](https://arxiv.org/abs/2305.09781)) — speedups only. SGLang's own
speculative-decoding docs publish throughput (158.34 → 244.10 → 373.25 tok/s for
none → EAGLE-2 → EAGLE-3 on Llama-3.1-8B, 1×H100, batch 1) but **no** acceptance numbers.
---

## 8. Retry, timeout and client behaviour

This section is thin, and that is the finding. Almost nothing is published about how real
clients behave against LLM APIs. What *is* available and citable is the **default behaviour
compiled into the official SDKs**, which is a genuinely good proxy because the overwhelming
majority of callers never change it.

### 8.1 SDK defaults, read from source

From `openai-python/src/openai/_constants.py` and
`anthropic-sdk-python/src/anthropic/_constants.py` on `main`, fetched 2026-09-06
[primary, read from source]:

| Constant | OpenAI Python SDK | Anthropic Python SDK |
|---|---|---|
| `DEFAULT_TIMEOUT` total | **600 s** (`# default timeout is 10 minutes`) | **600 s** (`10 * 60`) |
| connect timeout | **5.0 s** | **5.0 s** |
| `DEFAULT_MAX_RETRIES` | **2** | **2** |
| `INITIAL_RETRY_DELAY` | **0.5 s** | **0.5 s** |
| `MAX_RETRY_DELAY` | **8.0 s** | **8.0 s** |
| `MAX_RETRY_AFTER_DELAY` | **120 s** | not present |
| `max_connections` | **1000** | **1000** |
| `max_keepalive_connections` | **100** | **100** |

Both are Stainless-generated from the same template, hence identical values. (Both now
depend on `httpx2`, a fork, rather than `httpx` — relevant only if you plan to instrument the
transport for calibration.)

Widening to the other common clients changes the picture materially:

| Client | Default timeout | Default retries | Notes |
|---|---|---|---|
| openai-python / openai-node | 600 s (connect 5 s) | 2 | as above |
| anthropic-python / anthropic-ts | 600 s (connect 5 s) | 2 | as above |
| **google-genai (Python and JS)** | **none** | **0** | `retry_args(None)` returns `{'stop': tenacity.stop_after_attempt(1)}`, i.e. one attempt. The `_RETRY_ATTEMPTS = 5` constants are fill-ins that apply only once a caller opts in with an `HttpRetryOptions`. `HttpOptions.timeout` defaults to `None`. [primary, verified from source 2026-09-06] |
| **google-cloud-aiplatform** (Vertex) | **none** | **0** | every inference transport method wrapped with `default_timeout=None`; no `default_retry` in the file [secondary] |
| **langchain-openai `ChatOpenAI`** | **none** | 2 (inherited) | `request_timeout` defaults to `None` and is passed **unconditionally** into `client_params["timeout"]`, while `max_retries` is passed only `if not None`. Because the Stainless clients gate on a `NotGiven` sentinel (`if not is_given(timeout): timeout = DEFAULT_TIMEOUT`), an explicit `None` **is** "given" and the 600 s default is skipped — so `timeout=None` reaches httpx and means *no timeout at all*. [primary, verified from source 2026-09-06] |
| **langchain-anthropic `ChatAnthropic`** | **none** | 2 (explicit field) | same mechanism |
| litellm `completion()` | 600 s | 0 (`num_retries=None`); the underlying provider client still gets 2 | jitter is **additive** `U(0, 0.75) s`, not multiplicative [secondary] |
| litellm Router | `litellm.request_timeout` (**6000 s**) | 2 | *"For RateLimitError we implement exponential backoffs. **For generic errors, we retry immediately.**"* [secondary, litellm docs] — a zero-delay retry path is a retry-storm mechanism |

**Three of the most widely deployed client configurations have no request timeout at all.**
That is a materially different failure mode from a 600 s deadline: a hung request never
returns and never retries. A simulator should model "client waits forever" as a real state.

### 8.2 The exact retry algorithm

From `openai-python/src/openai/_base_client.py`, `_calculate_retry_timeout` and
`_should_retry`, fetched 2026-09-06 [primary, read from source]:

```
if Retry-After header present and 0 < value <= 120 s:  sleep exactly that
else: sleep = min(0.5 * 2^attempt, 8.0) * (1 - 0.25 * random())
```

Retry is attempted on **408** (request timeout), **409** (lock timeout), **429** (rate
limit), and **any 5xx**. A non-standard `x-should-retry: true|false` response header
overrides everything. A `Retry-After` greater than 120 s suppresses the retry entirely. Each
retry carries an idempotency key of the form `stainless-python-retry-<uuid4>`.

Two facts a simulator should encode:

- **Jitter is only ±25 % downward** (`1 - 0.25 * random()`, so the multiplier is in
  [0.75, 1.0]). This is *not* full jitter. Retries from a synchronised burst of clients stay
  substantially correlated, which is exactly the condition for a retry storm. Compare
  AWS-style full jitter, `random() * cap`, which decorrelates.
- **The default retry budget is small (2) but the timeout is enormous (600 s).** So the
  amplification factor is at most 3× total attempts, but a hung request occupies a client
  slot for ten minutes. For a simulator the second fact matters more: with a 600 s client
  timeout, **the client will essentially never give up before the server does**, so
  server-side queue-drop and admission control, not client timeout, determine what happens
  under overload.

Total retry delay from the default schedule is therefore only about
0.5 + 1.0 = **1.5 s worst case** before the attempts are exhausted [inferred] — trivial
compared with a 600 s timeout. The retry-driven load amplification that matters is not the
SDK's own 3×; it is **layering**. Azure's own quota documentation warns about it explicitly:
*"When using a custom retry library, set `max_retries=0` on the SDK client to disable its
built-in retry. Otherwise, each attempt from tenacity might itself trigger up to two
additional SDK retries, leading to far more requests than expected."* [secondary,
learn.microsoft.com Azure AI Foundry quota page] With a typical `stop_after_attempt(6)`
wrapper on top of the SDK's 2, that is **up to 18 requests per logical call** — a documented
6× amplification over the intended budget, and the correct upper end for a retry-storm
scenario.

Two other published behaviours worth encoding:

- **OpenAI's ramp-rate rule**, which is effectively a published statement about how fast a
  fleet can absorb growth: *"once your traffic reaches 1 million input tokens per minute
  (TPM), increase it by no more than 50% every 15 minutes."* [secondary,
  developers.openai.com rate-limits guide] Also: *"unsuccessful requests contribute to your
  per-minute limit"*, so retries against a 429 make the 429 worse.
- **Anthropic's rate limiter is a token bucket** — *"your capacity is continuously
  replenished up to your maximum limit, rather than being reset at fixed intervals"*
  [secondary] — which is the right shape for a simulated admission controller. And one 429
  class is deliberately **retry-proof**: the spend-cap 429 carries no `retry-after`, and
  *"Retrying, including the SDKs' automatic retries, fails until access resumes."*
  [secondary] A simulator modelling 429s should distinguish retryable from terminal ones,
  because auto-retry burns the whole budget on the terminal class.

### 8.3 What is not published

| Quantity | Status |
|---|---|
| Distribution of client-configured timeouts in the wild | **not found** |
| Retry counts actually configured in production deployments | **not found** |
| Measured retry amplification factor during a real LLM API incident | **not found** — no published postmortem quantifies it |
| Fraction of requests cancelled mid-stream by users | **not found**. Discussed qualitatively (users edit and resend, voice barge-in, "stop" buttons) but no source publishes a rate |
| Whether gateways actually propagate client disconnect to `abort(request_id)` | **not found** as a measured practice; vLLM exposes `abort(request_id)` but whether it is called is a deployment question |
| Fraction of requests that fail / are rejected server-side | **7.56 %** for BurstGPT_3 API log and **0.79 %** for its Conversation log, as `Response tokens == 0` [measured-here]. This is the only public per-request failure rate we found. BurstGPT's paper attributes it to KV-cache pressure: *"average failure rate of the conversation service is consistently high, exceeding 5% for ChatGPT"* and *"Variations in burstiness in BurstGPT lead to memory bottlenecks, causing spikes in failure rates."* [primary] |

The BurstGPT failure rate is worth flagging: a **7.6 % request failure rate on API traffic**
is far higher than a distributed-systems engineer would expect from a managed service, and it
is correlated with burstiness. If real, it means retry traffic is a non-trivial fraction of
offered load and belongs in the model. Treat the specific value as one service's 2023-era
measurement, not an industry norm.

---

## 9. What is NOT public

These are quantities the simulator needs and nobody publishes. Each gets a **[GUESS]**
default and a sweep range. Mark every one of them as an assumption in the config, not a
calibrated constant.

### 9.1 Workload

| Quantity | Why it matters | Default [GUESS] | Sweep range | Notes |
|---|---|---|---|---|
| **Fleet-level continuation fraction** (share of requests that are turn ≥ 2) | Sets the ceiling on session-driven cache reuse | **0.25** | **0.03 – 0.76** | Endpoints of the range are measured (§4.2); the fleet mix is not. |
| **Prefix-sharing topology** — how many distinct system-prompt roots, and their popularity distribution | Decides whether affinity routing hot-spots | **Zipf(s≈1.0) over ~100 roots, top root = 30 % of traffic** | 1 root (Mooncake) to 10⁴ roots | Only Mooncake reveals root count (1 and 4), and its sample is one hour of one service. This is the biggest single unknown. |
| **Joint distribution of (input length, output length, session position, application id)** | Everything downstream | independent sampling with ρ(in,out) = 0.4 for chat, 0.0 for code | ρ ∈ [0, 0.5] | Only marginals and one correlation are public. |
| **Burst duration and inter-burst time** | Autoscaler evaluation | **burst duration 30 s, exponential; inter-burst 600 s** | duration 5 – 300 s | Not found anywhere. Our IDC saturation (§2.4) weakly supports 10–60 s. |
| **Cancellation / abandonment rate and cancellation-time distribution** | Wasted decode; a real capacity leak | **2 % of streaming requests, cancelled at Uniform(0.2, 0.8) of output** | 0 – 10 % | Not found. Qualitatively discussed, never measured publicly. |
| **Retry-driven share of offered load** | Overload dynamics | **3 % at steady state, up to 40 % during an incident** | 0 – 100 % | Inferable only from the SDK defaults (§8.2) plus BurstGPT's 7.6 % failure rate. |
| **Per-tenant / per-client rate distribution** | Whether a few tenants dominate | **Zipf: top 1 % of clients = 90 % of requests** | top 1 % = 50 – 99 % | ServeGen is the one data point: *"top 29 clients responsible for 90% of requests"* of 2,412 clients [primary] ⇒ top 1.2 % = 90 %. Whether that generalises is unknown. |
| **Model-mix distribution across a heterogeneous fleet** | Multi-model routing | **3 models, 70/25/5** | — | FineServe (57 models) and Chutes (9,174 models) characterize it but publish no usable mix table. |
| **SLO class mix** (interactive / batch / background share) | Goodput is defined per class | **70 % interactive, 20 % batch, 10 % background** | interactive 40 – 95 % | **Nothing public at all.** Not one trace carries an SLO or priority label. |
| **Reasoning-token share of output** | Reasoning models change the in:out ratio | **for reasoning models, reasoning ≈ 4× answer length** | 1× – 10× | ServeGen: *"reason lengths can be on average 4× longer than answer lengths"*, bimodal [primary]. TraceLab has a `reasoning_output_tokens` field, so this is measurable from public data — we did not. |

### 9.2 System and policy

| Quantity | Why it matters | Default [GUESS] | Sweep range |
|---|---|---|---|
| **Actual production prefix-cache TTL and eviction policy** | §4.3 shows TTL sits on the knee of the think-time distribution | **LRU, 5 min TTL** | 1 min – 1 h |
| **Router metric staleness in production** | The herding mechanism in primer §6 | **5 s scrape interval** | 1 – 30 s (primer says 1–15 s) |
| **Number of independent routers** | Herding amplitude scales with it | **8** | 1 – 64 |
| **Admission control / queue-drop policy at overload** | Determines whether overload degrades or collapses | **drop at queue depth > 4× service capacity** | — |
| **Real preemption rate under production load** | Recompute vs swap cost | **not found; assume 1 % of requests preempted at 80 % KV utilization** | 0 – 20 % |
| **KV cache utilization distribution in production** | Where on the latency knee real fleets operate | **p50 60 %, p99 92 %** | — |
| **Warm-pool size actually held in production** | The cost/latency tradeoff in primer §7 | **10 % of steady-state replica count** | 0 – 50 % |
| **Real fleet size, replicas per model, GPU generation mix** | Everything about scale | — | — |

**Not one public source gives a fleet size, a replica count, an SLO target, a queue-depth
distribution, or a KV-utilization distribution.** DeepSeek's 226.75 average H800 nodes is the
closest thing to a published fleet size, and it is a single 24-hour snapshot of one service.

### 9.3 The honest summary

The public record gives you, with confidence: **input and output length marginals**,
**arrival timestamps at fleet scale for two services**, **turn counts and think time for one
chat service and one agentic corpus**, and **prefix-reuse ceilings for one long-context
service**. It gives you nothing at all about **SLO classes, cancellation, retry behaviour,
fleet sizes, or the sharing topology of system prompts**. Design the simulator so those five
are explicit, named, sweepable parameters that appear in every result caption, because they
are assumptions and a reader must be able to see them.

---

## 10. Recommended starting workload config

Mirroring the style of `docs/llm-serving-primer.md` §10.7. Three scenarios, because a single
workload config would hide the very variation that matters. Every value is traceable to a
section above; **[GUESS]** marks values from §9.

```yaml
# ---------------------------------------------------------------------------
# Scenario A: "chat-2024" — calibrated to Azure LLM inference trace 2024, conv
# Use for: baseline, arrival-process work, autoscaling. CC-BY, replayable.
# ---------------------------------------------------------------------------
workload_chat_2024:
  provenance: "AzureLLMInferenceTrace_conv_1week.csv, 27,303,999 req, 168 h, CC-BY"
  arrivals:
    mode: replay                      # prefer replay; the synthetic block below matches it
    mean_rate_rps: 45.15              # measured
    synthetic:
      model: mmpp                     # NOT Poisson; see section 2.1
      diurnal_peak_over_trough: 1.78  # measured, hour-of-day averaged, UTC
      diurnal_peak_hour_utc: 14
      # rate multiplier is a mean-reverting process tuned so that, in a fixed hour:
      #   IDC(T=1s) ~= 1.6, IDC(T=10s) ~= 5.5, IDC(T=60s) ~= 17
      target_idc: {1: 1.6, 10: 5.5, 60: 17.3}   # measured
      interarrival_cv_5min_window: 1.64          # measured, cross-check
  lengths:
    input:
      sampler: empirical_quantiles    # do not fit a family; see section 3.3
      mean: 1632
      p50: 928
      p90: 3830
      p99: 6683
      max: 7999                       # NOTE: trace is 8k-truncated. Not for long context.
      cv: 0.94
    output:
      sampler: lognormal
      mu: 3.660                       # measured, log-moment fit
      sigma: 1.491
      mean: 105.5
      p50: 41
      p99: 694
      cv: 1.50
    corr_input_output: 0.392          # measured
  sessions:
    # Azure trace has no session ids; structure borrowed from BurstGPT_3 conv
    provenance_note: "session structure from BurstGPT_3, not from the Azure trace"
    turns_pmf: [0.355, 0.204, 0.119, 0.076, 0.051, 0.037, 0.028, 0.021, 0.018, 0.013]
    turns_tail: zeta                  # 7.8% of conversations have >10 turns
    turns_mean: 4.18
    continuation_fraction_within_chat: 0.761
    fleet_continuation_fraction: 0.25 # [GUESS] section 9.1
    think_time:
      sampler: lognormal
      mu: 5.18                        # measured; median 178 s
      sigma: 1.96
      p50_s: 131
      p90_s: 2340
    context_growth_tokens_per_turn: 187   # measured, mean; p50 125, p10 -124
  prefix_sharing:
    # not measurable from the Azure trace; borrowed from Mooncake conversation
    n_system_prompt_roots: 100        # [GUESS] section 9.1 — biggest unknown
    root_popularity: zipf
    root_zipf_s: 1.0
    root_prompt_tokens: 1024          # [GUESS]
    ideal_block_hit_rate_unbounded: 0.366   # measured, Mooncake conversation
  reliability:
    server_failure_rate: 0.008        # measured, BurstGPT_3 conv (response==0)
    cancel_rate: 0.02                 # [GUESS] section 9.1
    retry_share_of_offered_load: 0.03 # [GUESS] section 9.1

# ---------------------------------------------------------------------------
# Scenario B: "agent-2026" — calibrated to Mooncake toolagent + TraceLab
# Use for: prefix caching, P/D disaggregation, long-context, KV pooling.
# This is the scenario where 2026 conclusions differ most from 2024 ones.
# ---------------------------------------------------------------------------
workload_agent_2026:
  provenance: "Mooncake FAST25 toolagent_trace.jsonl (23,608 req, 1 h) + TraceLab (357k rounds)"
  arrivals:
    mode: replay_and_loop             # only 1 h of data; loop or superpose
    mean_rate_rps: 6.67               # measured; scale to your fleet
    synthetic:
      model: mmpp
      interarrival_cv_whole_hour: 4.36  # measured
      diurnal_peak_over_trough: 8.96    # [borrowed] Azure 2024 code, a dev-hours service
  lengths:
    input:
      sampler: empirical_quantiles
      mean: 8596
      p50: 6346
      p90: 16810
      p99: 61671
      max: 126195
      cv: 1.28
    output:
      sampler: lognormal
      sigma: 1.98                     # measured
      mean: 182
      p50: 30                         # note: median 30 tokens, mean 182 — very skewed
      p99: 898
      cv: 1.33
    corr_input_output: 0.0             # [GUESS]; not measurable from Mooncake
    input_output_ratio_of_means: 47    # measured
  sessions:
    provenance_note: "structure from TraceLab; Mooncake gives no session ids"
    requests_per_session_mean: 9.2    # TraceLab
    requests_per_session_p99: 137
    tool_calls_per_step_mean: 1.2
    session_duration_p50_s: 306       # 5.1 min
    session_duration_p90_s: 21240     # 5.9 h
    inter_step_gap:
      tool_latency_p50_s: 0.3
      tool_latency_p90_s: 13.6
      tool_share_of_response_time: 0.598
    context_growth_tokens_per_turn: 2242   # vLLM x Mooncake
    append_tokens_per_step_p50: 857         # TraceLab
  prefix_sharing:
    block_tokens: 512                 # Mooncake's own block size
    n_system_prompt_roots: 4          # measured
    root_share: [0.463, 0.390, 0.146, 0.001]  # measured
    ideal_block_hit_rate_unbounded: 0.553     # measured
    hit_rate_vs_cache_tokens:         # measured; the sizing curve
      0.51e6: 0.340
      2.05e6: 0.370
      4.10e6: 0.438
      8.19e6: 0.497
      16.4e6: 0.541
      32.8e6: 0.552
    hit_rate_random_routing:          # measured; the cost of cache-blind LB
      1: 0.553
      2: 0.492
      4: 0.439
      8: 0.400
      16: 0.374
    provider_reported_hit_rate: 0.957 # TraceLab, for a real coding-agent product
    truncation_penalty: 0.47          # 85% -> 45% cliff, LMCache; apply if client truncates
  reliability:
    server_failure_rate: 0.008        # [GUESS], borrowed
    cancel_rate: 0.02                 # [GUESS]
    retry_share_of_offered_load: 0.03 # [GUESS]

# ---------------------------------------------------------------------------
# Scenario C: "code-completion" — calibrated to Azure LLM 2024, code
# Use for: the extreme prefill-bound corner. 111:1 in:out.
# ---------------------------------------------------------------------------
workload_code_completion:
  provenance: "AzureLLMInferenceTrace_code_1week.csv, 16,803,695 req, 168 h, CC-BY"
  arrivals:
    mode: replay
    mean_rate_rps: 27.78
    synthetic:
      model: mmpp
      diurnal_peak_over_trough: 8.96          # measured, hour-of-day averaged
      diurnal_peak_over_trough_worst_hours: 33.1   # measured, single busiest/quietest hour
      diurnal_peak_hour_utc: 19
      target_idc: {1: 1.9, 10: 7.0, 60: 19.9} # measured
      interarrival_cv_5min_window: 1.15       # measured
  lengths:
    input:
      sampler: empirical_quantiles
      mean: 2511
      p50: 1930
      p90: 6251
      p99: 7685
      max: 7743
      cv: 0.85
      alt_parametric: {family: gamma, k: 1.385, theta: 1812.6}  # gamma wins here
    output:
      sampler: lognormal
      mu: 2.030
      sigma: 1.373
      mean: 22.7
      p50: 8
      p99: 271
      cv: 3.296                       # highest service-time variability of any trace
    corr_input_output: 0.000          # measured — exactly zero, unlike chat
    input_output_ratio_of_means: 111  # measured
  sessions:
    fleet_continuation_fraction: 0.0  # [GUESS]; inline completion is stateless per request
  prefix_sharing:
    # HumanEval-style code completion measures 0.1% hit rate (SwiftCache).
    # But a real IDE resends the same file repeatedly, so the truth is in between.
    ideal_block_hit_rate_unbounded: 0.20   # [GUESS]; range 0.001 - 0.6

# ---------------------------------------------------------------------------
# Engine efficiency constants — see section 6. These REPLACE the corresponding
# entries in the primer's section 10.7 `engine:` block, which double-counts
# overhead between mbu_decode and fixed_step_overhead_s (section 6.2).
# ---------------------------------------------------------------------------
engine_calibration:
  # Convention: MBU is a PURE bandwidth-efficiency number; all overhead lives
  # in t_fixed and t_collective. Do not lower MBU at batch 1 as well.
  mbu_decode: 0.80                    # megakernel 78%, FlashFormer 82%; range 0.75-0.85

  mfu_prefill_dense: 0.50             # Llama-3-70B/405B measured 0.50-0.63 at long prompts
  mfu_prefill_moe: 0.25               # MoE measured 0.16-0.36 — a 2-3x split
  mfu_prefill_short_chunk_curve:      # GEMM-only upper bound, H100 fp16, N=K=8192
    128: 0.36
    256: 0.65
    512: 0.78
    1024: 0.81
    2048: 0.82
  fp8_prefill_gain_over_bf16: 1.4     # NOT 2.0; GEMM tops out at 56-58% of doubled peak

  # CALIBRATE THIS FIRST, against one batch-1 measurement.
  fixed_step_overhead_s: 0.0027       # modern engine w/ CUDA graphs (back-derived from NIM)
  fixed_step_overhead_s_no_graphs: 0.008   # vLLM <=0.5.3 class; the primer's value
  fixed_step_overhead_moe_multiplier: 5    # TaxBreak: 8-11x the kernel count

  tp_allreduce_us_per_collective: 9   # TP8 custom kernel, <64KB. NCCL is ~20. Range 4-23.
  collectives_per_layer: 2            # confirmed: 160 all-reduces per 80-layer forward pass

  # Validation targets, all measured (section 6.5):
  validate_batch1_70b_tp8_bf16_ms: 10.25    # NIM, 8xH100, ISL/OSL 1000/1000
  validate_batch1_8b_tp1_fp8_ms: 4.53       # NIM, 1xH100
  validate_maxthroughput_70b_tp8_fp8_toks: 11082   # TRT-LLM, 1000/1000
  accuracy_target_tpot_pct: 10        # published simulators achieve ~9-10% TPOT MAPE
  accuracy_target_ttft_pct: 22        # and ~22% TTFT MAPE. Do not chase 3%.
```

### Sanity checks this config should reproduce

Extending the primer's §10.7 checklist with workload-side checks:

- **Arrival burstiness.** In a fixed simulated hour, the index of dispersion of arrival
  counts should be ≈ 1.5–2 at T = 1 s and ≈ 17–20 at T = 60 s. If your generator gives IDC ≈ 1
  at T = 60 s, it is Poisson and you will understate overload episodes by an order of
  magnitude.
- **Diurnal swing.** Scenario A should swing 1.8× over a day; scenario C should swing ~9×.
- **Prefill:decode work ratio.** Scenario A at 15.5:1 input:output should be roughly
  balanced between prefill and decode FLOP-seconds; scenario C at 111:1 should be almost
  entirely prefill; scenario B at 47:1 with 8.6k-token prompts should be prefill-dominated
  but with long KV residency.
- **Prefix hit rate at realistic cache size.** With scenario B and a per-replica KV cache of
  1.37 M tokens (primer §10.7), the achieved block hit rate should land near **0.35**, and
  random routing across 8 replicas should drop it toward **0.40 × (capacity factor)**. If
  your simulator reports 90 %+, the cache model is wrong.
- **Service-time spread.** p99/p50 of end-to-end latency should exceed 10× in scenario C,
  driven by the 3.3 output-token CV.
- **Think-time / TTL interaction.** With a 5-minute prefix-cache TTL and scenario A's think
  time, about **two thirds** of continuations should hit a live cache entry and one third
  should miss. If your simulator shows 95 % of continuations hitting, the think-time
  distribution is too short.
- **Session concurrency.** Scenario B's median session lasts 5.1 min with 9.2 requests and
  spends 59.8 % of wall time in tool execution, so the number of *live sessions* should be
  roughly an order of magnitude larger than the number of *in-flight requests*. That gap is
  where session-state cost lives.

And the engine-side checks, all against measured values in §6.5:

- **Batch-1 decode, 70B on 8×H100 bf16:** **10.25 ms per token, 97 tok/s.** If your model says
  15 ms, `t_fixed` is set for a 2024 engine.
- **Batch-1 decode, 8B on 1×H100 fp8:** **4.53 ms per token, 220 tok/s.**
- **Batch-1 TP scaling, 70B:** 53 → 71 → 97 tok/s for TP2 → TP4 → TP8. Sublinear by design;
  if your model is linear, `t_collective` is missing.
- **Max throughput, 70B FP8 on 8×H100:** **11,082 output tok/s** at 1000/1000 and **8,773** at
  2048/2048.
- **Prefill chunk budget:** an 8192-token budget should show no throughput penalty on H100 but
  a visible one on A100, whose 4096×4096 GEMM saturates near 2048 tokens.
- **Accuracy bar:** stop when TPOT is within ~10 % and TTFT within ~22 % of a measured
  reference. That is what published simulators of this kind achieve.

---

## 11. Sensitivity: what to vary

Ranked by how much the parameter moves conclusions, most to least. The first four should
never be fixed constants in any published result.

### Tier 1 — sweep always; conclusions invert across the range

| Parameter | Range | Why it inverts conclusions |
|---|---|---|
| **Input:output ratio** | **1:1 → 130:1** (§3.2) | Decides whether the fleet is prefill- or decode-bound, and therefore whether P/D disaggregation, chunked prefill, or decode batching wins. Every published result on ShareGPT (1.1:1) is silent about agentic traffic (100:1). |
| **Arrival burstiness at the 10–60 s timescale** (IDC(60 s)) | **1 → 20** (§2.1) | Poisson vs measured is a 20× difference in variance at the timescale that governs queueing and autoscaling. Determines whether a policy that looks fine at the mean collapses under bursts. |
| **Ideal prefix reuse fraction and cache capacity** | reuse **0.001 → 0.96**, cache **0.5 M → 50 M tokens** (§5.3, §5.4) | The whole locality-vs-load tradeoff. At reuse 0.05 affinity routing is pointless; at 0.95 it is the only thing that matters. And a 1.37 M-token per-replica cache realises 0.06 of a 0.37 ceiling on long-context chat — cache tier, not workload, drives that. |
| **Fixed per-step overhead** `t_fixed` | **0.5 → 8 ms** (§6.1) | Sets small-batch performance, and therefore the marginal value of consolidating load. It is the whole difference between a 2024 engine (~8 ms, 62 % of the step on CPU) and a 2026 one (~2.7 ms). At batch 1 a decode step is measurably **majority host work** (HDBI 0.25). Calibrate against a batch-1 measurement before anything else. |

### Tier 2 — sweep for any conclusion about routing or autoscaling

| Parameter | Range | Effect |
|---|---|---|
| **Fleet continuation fraction** | 0.03 → 0.76 (§4.2) | Sets how much reuse is session-chain (scatters under LB) vs shared-root (does not). |
| **Number of system-prompt roots and their popularity skew** | 1 root → 10⁴ roots (§9.1) | With 1 root, greedy affinity routing degenerates to one hot replica — we measured exactly that (§5.4). With many roots it balances. This is the largest **unmeasured** lever. |
| **Prefix-cache TTL vs think time** | TTL 1 min → 1 h against p50 think time 0.3 s → 131 s (§4.3, §5.3) | TraceLab's own sweep: 85.4 % @ 1 min → 98.6 % @ 1 h. Our think-time median of 131 s sits right on the 5-minute knee. |
| **Diurnal amplitude** | 1.8× → 33× (§2.3) | Determines whether autoscaling matters at all. A global chat service barely needs it; a regional coding assistant needs it badly. |
| **Router metric staleness and router count** | 1–30 s, 1–64 routers (§9.2) | The herding mechanism in primer §6. Entirely unmeasured in public. |
| **Output-length CV** | 0.7 → 3.3 (§3.4) | Sets queueing delay under any non-SJF policy, and the value of output-length prediction. |

### Tier 3 — sweep when the specific mechanism is under study

| Parameter | Range | When it matters |
|---|---|---|
| Retry share of offered load | 0 → 100 % | Overload and incident scenarios only. Note the SDK's ±25 % jitter keeps retries correlated (§8.2). |
| Cancellation rate | 0 → 10 % | Wasted-decode accounting; capacity headroom. |
| corr(input, output) | 0 → 0.5 (§3.5) | Whether expensive prefills also produce expensive decodes; affects work-estimation routing. |
| Speculative-decoding α and K, with speedup emergent | α 0.35–0.85, K 0–7 (§7) | Only if speculative decoding is in scope. Sweep the **crossover batch size** (observed 16 to > 200) rather than assuming one. |
| MBU / MFU | MBU 0.75–0.85; MFU **dense 0.40–0.63 vs MoE 0.16–0.36** (§6.2, §6.3) | Scales absolute throughput but rarely changes policy *ordering*. The exception is the **dense-vs-MoE MFU split**, which is a 2–3× difference and does change where the prefill/decode balance falls. |
| TP degree at low batch | TP2–TP8 (§6.5) | Batch-1 TP scaling is strongly sublinear (4× GPUs buys 1.8× speed), so consolidation-versus-spreading conclusions depend on it. |
| Server failure rate | 0.8 % → 7.6 % (§8.3) | Reliability scenarios. |
| Model mix across a heterogeneous fleet | — | Multi-model routing studies. |
| Truncation penalty on hit rate | 0 → 0.47 (§5.6) | Whenever clients manage context with a sliding window. |

### What not to bother varying in v1

- Prompt content or tokenizer choice. Only token counts matter.
- Multimodal image counts, unless multimodal routing is a goal (Azure LMM 2025 exists if it is).
- Reasoning-vs-answer token split, unless reasoning models are a named scenario — though note
  ServeGen's 4× reason:answer ratio makes this a bigger deal than it looks.
- The exact parametric family for input length. Sample from the empirical quantiles; the
  family choice is a worse approximation than the trace itself (§3.3).

---

## Appendix: reproducing the [measured-here] numbers

Every number tagged **[measured-here]** came from downloading the primary dataset and
computing it. The datasets:

```bash
# Azure LLM inference traces 2024 (CC-BY). ~1.8 GB total.
curl -L -O https://github.com/Azure/AzurePublicDataset/releases/download/dataset-llm-2024/AzureLLMInferenceTrace_code_1week.csv
curl -L -O https://github.com/Azure/AzurePublicDataset/releases/download/dataset-llm-2024/AzureLLMInferenceTrace_conv_1week.csv

# Azure LLM inference traces 2023 (CC-BY). Small.
curl -L -O https://raw.githubusercontent.com/Azure/AzurePublicDataset/master/data/AzureLLMInferenceTrace_code.csv
curl -L -O https://raw.githubusercontent.com/Azure/AzurePublicDataset/master/data/AzureLLMInferenceTrace_conv.csv

# BurstGPT (CC-BY-4.0). BurstGPT_3 is the only one with Session ID.
curl -L -O https://github.com/HPMLL/BurstGPT/releases/download/v2.0/BurstGPT_3.csv

# Mooncake FAST'25 traces (license not stated). The only trace with prefix block hashes.
for f in conversation_trace toolagent_trace synthetic_trace; do
  curl -L -O https://raw.githubusercontent.com/kvcache-ai/Mooncake/main/FAST25-release/traces/$f.jsonl
done

# TraceLab (dataset CC BY 4.0, code Apache-2.0).
curl -L -O https://github.com/uw-syfi/TraceLab/releases/download/v0.0.1/syfi_coding_trace.jsonl.gz
```

Analyses performed: per-column moments and exact integer-histogram percentiles; Pearson
correlation of input against output tokens; lognormal fit by log-moments and gamma fit by
moments, each scored with a KS statistic against the empirical CDF; interarrival CV inside
detrended 5-minute windows; index of dispersion for counts at T ∈ {0.1, 0.5, 1, 5, 10, 60} s
inside single fixed hours; hour-of-day arrival profiles; session grouping by `Session ID`
with turn counts and inter-turn gaps; and longest-matching-prefix cache simulation over
Mooncake's 512-token `hash_ids` with unbounded and LRU-bounded capacity and with
random / greedy-affinity routing across N replicas.

These are all a few dozen lines of standard-library Python. Re-derive them rather than
trusting this file — that is the point of tagging them.
