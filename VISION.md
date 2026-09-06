# VISION

> Fill each section in a few sentences. Bullet fragments are fine. Delete prompts as you go.
> Sections marked (Claude) are for me to fill after you have written yours.

## 1. One-line pitch

<!-- What is this, in one sentence, for someone who runs an inference fleet? -->

## 2. Why this exists

<!-- What can't you learn from a real cluster, a load test, or a spreadsheet? What decisions
     will this simulator change? Who is the user: you, a platform team, researchers? -->

## 3. The two goals, made concrete

### 3a. Understand real-world dynamics
<!-- Which phenomena must the simulator reproduce before you trust it? Examples to keep or cut:
     - decode latency spikes when a long prompt prefills into a running batch
     - KV cache pressure -> preemption cascades
     - retry storms / metastable overload after a partial outage
     - autoscaling lag vs. diurnal and bursty traffic
     - prefix-cache locality vs. load spreading
     - head-of-line blocking across tenants / SLO classes -->

### 3b. Evaluate policies
<!-- Which policy families matter most, in priority order?
     load balancing / routing, replica scheduling, admission & traffic shaping,
     autoscaling, retry/backoff, failover, prefill-decode disaggregation... -->

## 4. What "realistic" means here

<!-- What ground truth will you calibrate against? Published vLLM/SGLang numbers, your own
     traces, a specific model+GPU pair (e.g. Llama-3-70B on 8xH100)? How much fidelity do you
     need: token-level engine steps, or request-level queueing approximations? -->

## 5. Non-goals

<!-- What is explicitly out of scope for v1 (and maybe forever)?
     e.g. modeling GPU kernels, network packets, multi-region, cost/billing, training workloads -->

## 6. Success criteria

<!-- How do you know it works? e.g.
     - reproduces phenomenon X qualitatively with default config
     - can rank 3 load-balancing policies on a fixed scenario in < 1 minute
     - a new policy is < 50 lines and needs no engine changes -->

## 7. Key questions you want answered first

<!-- The first 3-5 experiments you would run the day it works. -->

## 8. Constraints and preferences

<!-- Language, dependencies, runtime budget per experiment, reproducibility (seeds),
     config format, how results should be presented (CSV, plots, notebook, HTML report) -->

## 9. Architecture sketch (Claude)

<!-- I will fill this after sections 1-8 and after your wire diagram exists. -->

## 10. Open questions

<!-- Anything you are unsure about. I will answer or propose defaults. -->
