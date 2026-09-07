# Three stages, so the runtime image carries a binary and static files and nothing else.
#
# Node builds the dashboard, Rust builds the simulator and then *runs* it to generate the reports and
# the recorded demo runs, so the image ships real results rather than placeholders. The runtime stage
# has no toolchain, no package manager and no shell scripts.

# --- 1. the dashboard -------------------------------------------------------
FROM node:22-slim AS web
WORKDIR /w
# npm ci, not npm install: it installs exactly the lockfile and fails if the two disagree, so the
# image cannot silently pick up a different dependency tree than the one that was tested.
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- 2. the simulator, and the reports it produces --------------------------
FROM rust:1-slim-bookworm AS build
WORKDIR /s
# The whole context, rather than an enumerated list of source paths. The crate layout is not stable:
# docs/ARCHITECTURE.md 10.8 splits the single crate into a workspace under crates/, and an enumerated
# COPY breaks on the day that lands, with a Cargo manifest error that does not point at the Dockerfile.
# .dockerignore already keeps target/, .git, web/node_modules and the test and bench trees out, so the
# context is about 350 KB. The cost is layer caching on the Rust stage, which is worth nothing here:
# the crate has no external dependencies and builds in seconds.
#
# --locked is meaningful only because Cargo.lock is now in the context. It was not before, so the flag
# failed on every build and the build fell through to an unlocked one without saying so.
COPY . .
RUN cargo build --release --locked
# Generate the reports at build time. Telemetry is opt-in and stays off here: the image should carry
# what a reader looks at, not a few megabytes of CSV nobody asked for.
RUN mkdir -p site/reports \
 && ./target/release/sim-run compare scenarios/route_round_robin.txt scenarios/route_p2c.txt \
      scenarios/route_least_requests.txt scenarios/route_random.txt --out site/reports/1-routing.html \
 && ./target/release/sim-run sweep scenarios/route_least_requests.txt \
      --over telemetry_interval_ms=100,250,500,1000,2000,4000 --out site/reports/2-staleness.html \
 && ./target/release/sim-run sweep scenarios/route_p2c.txt \
      --over step_token_budget=512,1024,2048,4096,8192,16384 --out site/reports/3-chunking.html \
 && ./target/release/sim-run sweep scenarios/route_p2c.txt \
      --over arrival_rps=30,70,110,150,190,230 --out site/reports/4-load-curve.html \
 && ./target/release/sim-run sweep scenarios/route_p2c.txt \
      --over long_probability=0.0,0.04,0.08,0.16,0.32 --out site/reports/5-long-context.html \
 && ./target/release/sim-run compare scenarios/retry_none.txt scenarios/retry_budget.txt \
      scenarios/retry_storm.txt --out site/reports/6-retry.html \
 && ./target/release/sim-run compare scenarios/route_round_robin_no_decode.txt \
      scenarios/route_p2c_no_decode.txt --out site/reports/7-no-decode.html \
 && ./target/release/sim-run compare scenarios/admit_accept_all.txt \
      scenarios/admit_deadline_aware.txt --out site/reports/8-admission.html \
 && ./target/release/sim-run compare scenarios/admit_tenants_accept_all.txt \
      scenarios/admit_fair_share.txt --out site/reports/9-fair-share.html \
 && ./target/release/sim-run compare scenarios/route_p2c.txt \
      scenarios/route_least_kv_probe.txt --out site/reports/10-probes.html \
 && ./target/release/sim-run compare scenarios/kv_spiral_never.txt \
      scenarios/kv_spiral_swap.txt --out site/reports/11-preemption.html \
 && ./target/release/sim-run compare scenarios/spec_off.txt \
      scenarios/spec_n4.txt --out site/reports/12-spec-decode.html
# The recorded runs the dashboard replays (web/README.md, "Replay mode"): runs/index.json and one
# directory per run, written into site/ so the same stage owns everything the image serves. The
# dashboard fetches /runs/index.json relative to its own origin, so these have to sit beside the app
# in the runtime image rather than on a bucket. About 15 MB of JSON and two seconds of simulation,
# both measured, so it costs the build nothing worth caching. The scenario names inside the runs are
# the ones export.rs hard-codes in DEMOS; there is no flag to pick a subset, and none is wanted.
RUN ./target/release/sim-run export --demos --dir site

# --- 3. runtime -------------------------------------------------------------
FROM debian:bookworm-slim
RUN useradd --uid 10001 --create-home app
COPY --from=build /s/target/release/sim-run /usr/local/bin/sim-run
COPY --from=build /s/site/reports/ /srv/reports/
COPY --from=build /s/site/runs/ /srv/runs/
COPY --from=web /w/dist/ /srv/
COPY docs/findings.md /srv/docs/findings.md
USER 10001
ENV PORT=8080
EXPOSE 8080
# The server reads PORT from the environment, which is Cloud Run's contract.
ENTRYPOINT ["/usr/local/bin/sim-run", "serve", "--dir", "/srv"]
