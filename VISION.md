# VISION

## 1. One-line pitch

- Build a simulator of a large scale production LLM serving system, focusing on reaching
     real world production scale with reasonable fidelity to reproduce insightful dynamics
- The two main goals are:
     - Help me learn about interesting challenges of LLM serving in the real world
     - Help me learn about publicly available information about state of the art LLM serving
     - Create a playground to teach others
     - Serve as a useful tool to evaluate policies across the stack, including gpu/host/node
     level scheduling, load balancing policies, affinity strategies, traffic shapping,
     redundancy policies (vocab: I will call of that "policies" in general), and its impact
     to throughput, slo, goodput, service quality (vocab: I will call these "scorecard metrics")

## 2. Why this exists

- This will clearly not be as high fidelity as experiments in a real world cluster itself
     or observations directly from production, but it will be much cheaper and faster to evaluate
- It will also be hermetic, which not only makes it much cheaper and faster, but also allows
     using it as a playground for LLM agents to evalute policies

## 3. The two goals, made concrete

### 3a. Understand real-world dynamics
- KV cache -> evaluate policies for affinity, memory tiering, RDMA within host, outside, etc.
     This means that the simulator needs to have some model of what data is loaded where, but we
     need to figure out how to do that in a scalable way, I haven't figured that out exactly.
- Time response to load balancing strategies. Real world LB decisions rely on stale machine state
     and therefore are prone to control theory style oscilatory patterns. I would like to be able
     to reproduce that kind of time/frequency domain dynamics, produce insights borrowing on
     control theory and demonstrate the value of policies that incorporate robust control logic.
- Demonstrate more basic LB falacies
     I would like to see early on the problem of what happens with round robin and a heterogeneous
     set of size of queries, and how you can add up with a "rolling hotspot" situation even when
     the cluster is well below its theoretical rated capacity.
     This can be simulated without the detailed prefill/decode LLM dynamics. The focus should be on
     LLM serving, but we should have some tuning knobs (e.g turning off decode) which can make the
     traffic look like stateless serving queries.
- Demonstrate what a real world global cascading failure looks like.
- tradeoff between gpu utilization and service latency when we tune how we manage decode/prefill
     workloads, as well as when we introduce traffic that can have large prefill workloads.
- Pathological situations where we have sufficiently large number of sessions requiring KV cache
     but relatively low qps, and we end up suffering from pathological kv cache preemption and 
     severe cluster performance degradation. Could connect this with a cascading failure scenario.
- In each of those scenarios, it is critical to demonstrate the value of traffic shaping at each
     layer.
- If we have time, caputuring the dynamics of model weight locality, particularly interesting
     if we are simulating MoE serving or users sending requiests for multiple models.
- In a global serving simulation, with traffic from multiple geographies with different diurnal
     patterns, being able to simulate auto scaling of the fleet to optimize for utilization with
     on demand resource allocation, while maintaining good quality of service and being able to
     effectively absorb unexpected spikes. Again, I would like to be able to think of this in
     terms of a roubst control theory problem, I think this is an under invested connection of
     different areas of domain knowledge.
- Managing traffic with different latency SLO requirements.
- Observe the value and cost of speculative decoding, with some stochastic model of the small
     model agreement with the larger model.
- Affinity decisions can help with performance in the "good case" scenario, but there are 
     pathological scenarios where a hot spot of machine fail over can lead to a cascading
     overload, capture that dynamic.

### 3b. Evaluate policies
- Policies of interest:
     - load balancing
     - batch scheduling, prefil batch sizing
     - speculative decoding
     - load forecasting
     - latency forecasting
     - machine failover
     - prefill-decode disaggregation
     - prefill-decode batch co-scheduling
     - affinity decisions. how sticky do we need to be.
     - load shaping decisions with stale cluster status information
     - auto scale.

## 4. What "realistic" means here

There is a range of options for this. I will throw some possibilities here, but I would like your
help to go through that list above, iron out what level of scale and fidelity is necessary to
reproduce those dynamics, and then call out 

- Batch level realism (decide a batch at a time in the GPU). Not sure about scalability of this.
- Batch level realism to the extend that this can be computed analytically. (i.e. a scheduling
     policy algorithm sees N requests and decides how to use the next M batches across a subset
     of those N requests, cost proportional to N, but not to the number of batches). I think we
     probably want this. In general, I think the principle should be that work at the GPU/machine
     level should scale with number of requests, but not size of the work in the requests.
- The engine can't work at the token level, it needs to be scaling with number of requests but
     in order to scale we may want to implement some kind of request cohorts (not totally fleshed
     out idea) so that we can treat the volume of requests in a given cohort more or less as
     scalar quantities instead of discrete single requests and therefore scale with number of
     cohort inputs per/second. Needs more thought to make sure this works still.
- Fluid 
     The lowest level of fidelity is treating the whole thing with a model of compute/bandwidth
     share across all requests, essentially the whole thing looking like a fluid/flow computation.
     This looks a little bit like the cohort idea above with cohorts = 1. My idea above was that
     cohort=infty is request level realism, cohort=1 is fluid-style simulation and the number of
     cohorts give you anything in between.
- Data locality dynamics
     - KV cache -> Model where the KV cache for any session is stored (HBM of which node), when
          it gets preempted, which machine in CPU RAM it is store (can we assume access cost for
          RDMA to CPU RAM vs local host is almost the same to only consider this at the cluster
          level?), which cluster it is store in SSD. There is a lot of complexity on most fidelity
          here, we need to consider the level of fidelity we want carefully.
     - KV cache prefix
          This becomes relevant when we have forked sessions, multi-agents, etc. modeling KV
          cache prefix would be ultimate realism, but I suspect the complexity of this approaches
          building a full virtual memory table across a whole cluster. Help me consider for
          feasibility and document it, but likely infeasible.
     - Model weights
          Model loading model weights when we have multiple models and MoE in our serving footprint.
          lower priority than KV cache realism.
- Stale status
     We should definitely model how the upper level policy layers (cluster admission, load balancing,
     anything above the single machine level) get updates from individual machines so we can capture
     the dynamics of making decisions with stale information. Even at the machine level, it is making
     decisions at time t to what it will do at time t+k so it is also a type of delayed actuator.
- "Turn up" delay.
     For the autoscale policy
- Failure discovery delay.
     A machine may fail silently and to the client look like it is running very slowly (think about
     a partial dead lock or network partition) need to be able to capture that form of failure vs a
     cleanly announced shutdown.

- Calibrate machine parameters against SOTA NVIDIA clusters shapes, and LLM workload, model 
     sizes, KV cache sizes against available published data. Make all of this user configurable.

## 5. Non-goals

- A part of the vision that is explicitly out of the scope for this v1 is to be able to hook in
     to a core RPC framework of a real world system to automatically captured load statistics as well
     as sampled detailed request serving traces to create a calibrated feedback loop for this model.
     Remember this when we do a showcase of how this could work in the real world, but don't build this.
- Out of scope if any level of fidelity in this simulator that makes it infeasible to simulate real world
     global cluster footprint within a relatively small simulator footprint.
- Let's ignore cost/billing.
- No training workloads. In the real world, do people actuallyl mix training and serving workloads?
- Modeling gpu kernels, network packet level is out of scope.

## 6. Success criteria

Identify the key dynamics that we want to be able to reproduce.
For each of them, be able to show the behavior happening in a dashboard with strong visuals at all layers
that are relevant for that dynamic. Whenever relevant, we should produce an A/B comparison (e.g. between
two different set of policies) that demonstrate the behavior

Output and interaction surfaces:
- Web points.
     - Home page listing what this is and all the surfaces a user can interact with this
     - Interactive Load test control that allows a user to select load shape, cluster shape, policies
          tune all parameters on the fly. This is the fully customizable playground for the engine we are
          building. This should have a panel with top level knobs to control, a panel with an observability
          later for all time series, cluster level, machine level, etc that may be relevant.
     - Showcase of the scenarios and dynamics we listed above.
          This should leverage the same dashboard above, display a list of "scenario" cards that have preloaded
          settings and a "scenario play" small popup that walks step by step of what needs to happen.
- standalone simulator that can serve as a playground for agents to evaluate different policies.

## 7. Key questions you want answered first

Let's first look at what it would take to simulate all the dynamics above, then based our scope estimation
stack rank them based on what we can do today. 

## 8. Constraints and preferences

Keep the core simulation as dependency light as possible

I want the simulator to be deterministic with a global seed used as a random number generator for any of the
stochastic pieces. We should also set this up to take periorical snapshots to make scroll back/replay easy.
Ideally the experience should enable the UI to have a "fast forward, rewind" functionality in the UI that is
reasonable.

We should be able to trace single requests (sampled uniformly and sampled for latency buckets) so that we can
go back and understand the flow of that requests across all machines.

I want to use proto for defining interfaces and APIs, and I want to either right or carefully review all interfaces

I want you to keep a file as TODO tasks for me as we go along.

The final results should present themselves as a standalone dashboard.

I would like to be able to scale to at least 2x realtime for 5 global clusters with 10k GPU each. But ideally 10x that.
We will likely deploy this as a sharded service to GCP, but I don't want something too big, maybe keep it to 10
backend replicas.

The backends should be implemented in RUST and the frontend should be node.js +react.

Results should be presented as a static HTML report, with interactive links to reply.

<!-- Language, dependencies, runtime budget per experiment, reproducibility (seeds),
     config format, how results should be presented (CSV, plots, notebook, HTML report) -->

## 9. Architecture sketch (Claude)

<!-- I will fill this after sections 1-8 and after your wire diagram exists. -->

## 10. Open questions

Scaling
- Not sure about the idea of request cohorts, that needs some workshopping, pressure testing, help me with that.

Separation of simluator engine and policy
- Ideally a full separation of simulator engine and policy should exist.
- In practice, I suspect that would be bad for performance, having the policy itself compute/advance the state of the
     machine allows the "scale with number of requests, not size of requests". That said, I want to make sure that
     the policy can't "cheat", so even if the policy is computing machine state, there should be a simulator engine
     layer that ensures that the output of the policy obeys our "physics" model (i.e. time moves forward, all the
     work that needs to be done is done, the limits of each hardward resource are obeyed)

Creating an batle arena for policies that agents can iterate on
- Sounds like a great idea. We need to validate realism first, but it would be great, after we have implemented
     something credible, to give free reigns to claude to improve upon some defined key metrics.