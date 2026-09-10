# LBSim Overview

Building a simulator of a global scale LLM serving cluster

- Live demo at [lbsim.ai]()
- [GitHub](https://github.com/issao/lbsim)
- [Self-link](https://github.com/issao/lbsim/blob/master/docs/overview.md)

## Inspiration

Running complex planet scale serving systems require a precise orchestration of policies at multiple layers (GPU/host level scheduling, load balancing and routing, cache affinity and eviction policies, admission control and capacity estimation, balacing multi-SLO loads - broadly calling these "policies") to achieve service quality, goodput and robustness goals.

A reasonably realistic simulation engine would allow for (1) deepening understanding of cluster dynamics under different policies and load shapes with fast paced experiments, (2) evaluate existing policies and potential new changes, (3) speed up development of novel strategies that balance various measurements of efficiency, service quality and robustness.

The ultimate vision is to build this as a full feedback loop from both a live production system as well as a production-sized load test environment, where sampled traces and statistics from the production system improved realism of the simulator, and insights from the simulator feed back into policies used in the real world cluster, with the potential to vastly accelerate development of cluster management policies.

## Goal

1. Help me build pragmatic knowledge about LLM-specific serving dynamics that I only really know about on paper (prefill/decode loads, GPU compute vs HBM bandwidth vs HBM storage bottlenecks)
2. Showcase specific non-obvious dynamics with a live simulation engine.
3. Evaluate and compare different policies.
4. (stretch, didn't get to) Create a sandbox arena that enables fast-paced AI driven development of policies that meet Efficiency, Service Quality and Robustness goals.

## Show don't tell

See it live:
- [Load test dashboard](https://lbsim.ai/#/dashboard) gives you a free form interface to play with it.
- [https://lbsim.ai/#/showcase](https://lbsim.ai/#/showcase) is my attempt to create step-by-step demonstration of reproducing interesting cluster dynamics.
- [https://lbsim.ai/#/] has several "reports" linked under the "replay" section that provide a static summary of real runs that attempt to reproduce different serving dynamic scenarios.

## Design

### Architecture for scale
- Design with global serving footprint scale target of 10K GPU clusters
- Scale target, measured scale. (Claude, can you fill in latest loadtest.)
- Proto interfaces designed for sharding the simulator in the machine dimension, keeping the bulk of the O(machine) work at that layer. For faster iteration we kept implementation as a single process backend for now.
- Scalable data flows. Subscription based observations designed for O(1) data flow between client and ingress, O(leaf replicas) data flow between ingress layer and leaves. Notably, data flow to and from the client stays constant with machine count, qps as well as simulation speed (simulation sec/wallclock sec).
- Leaf computation should scale with number of requests and working set, but not with the actual work (e.g. tokens, batches, etc) or number of machines. This is achieved by maintaining
batch level realism, but analytically advancing epochs for as many batches as the current "working set" in a given GPU. We did not implement any capability for work preemtion.

### Functionality
- The load test dashboard enables defining:
    - Load described by a mix of small and large queries, with a distribution of prompt and response sizes.
    - Routing policies
    - Cluster configuration
- Displays realtime monitoring data on the right panel.

### Metrics
- The more notable design decision is that metrics follow a subscription pattern with a lease and a time sample parameter, delegating most metric work that scales with machine count and
request count to the sim leaf, to keep a O(1) data flow to the client (assuming worker view uses bounded pagination)

### Fidelity.
- Caputured dynamics of GPU compute constraint, HBM bandwidth and HBM storage size (implemented), DRAM/SSD cluster level storage and bandwidth (interfaces designed but not functional).
- Telemetry delays at the admission and routing layers.
- (Claude, any key points in fidelity that I forgot?)

### Determinism
- Stochastic modeling of request load and shape, failure events, etc. But using a single global seed to maintain determinism.
- Enables using this a sandbox for a meaningful "policy regression test" engine.
- Enables interactive rewind using periodic snapshots.

## How much time did I spent?

I asked Claude to review the transcript and git commits to keep me honest. 6.1 hours on building this, not counting 1.9 hours of "ops time" (setting up and deploying to GCP, buying and configuring a domain name, etc), which totaled 9 hours. Time to write this doc was not included, and one last bug fix round I couldn't resist (lets call ~7 hours not including the ops time)

## Transparent AI use and Critical human input

This project made extensive use of Claude Code for development. The most critical human inputs were the VISION.md document, UI functionality description, target cluster dynamics to capture, overall technical architecture, simulation fidelity level, observability interface and a through review of the proto interfaces between the different layers. I did not perform a thorough human code review for the majority of the code generated.

## Biggest surprise and lesson learned

I wish that I could say I discover some novel cluster management dynamic I wasn't expecting, but it wasn't the case. The biggest surprise to me likely reflects my relative inexperience using claude to build real world systems. It wasn't until I requested to have an agent actively profiling and improving the development cycle, and asked the TL to keep an explicitly execution graph that things significantly sped up.
