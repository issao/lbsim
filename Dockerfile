# Three stages, so the runtime image carries a binary and static files and nothing else.
#
# Node builds the dashboard, Rust builds the simulator and then *runs* it to generate the reports, so
# the image ships real results rather than placeholders. The runtime stage has no toolchain, no package
# manager and no shell scripts.

# --- 1. the dashboard -------------------------------------------------------
FROM node:22-slim AS web
WORKDIR /w
COPY web/package.json web/package-lock.json* ./
RUN npm install --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- 2. the simulator, and the reports it produces --------------------------
FROM rust:1-slim-bookworm AS build
WORKDIR /s
COPY Cargo.toml ./
COPY src/ ./src/
RUN cargo build --release --locked 2>/dev/null || cargo build --release
COPY scenarios/ ./scenarios/
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
      scenarios/retry_storm.txt --out site/reports/6-retry.html

# --- 3. runtime -------------------------------------------------------------
FROM debian:bookworm-slim
RUN useradd --uid 10001 --create-home app
COPY --from=build /s/target/release/sim-run /usr/local/bin/sim-run
COPY --from=build /s/site/reports/ /srv/reports/
COPY --from=web /w/dist/ /srv/
COPY docs/findings.md /srv/docs/findings.md
USER 10001
ENV PORT=8080
EXPOSE 8080
# The server reads PORT from the environment, which is Cloud Run's contract.
ENTRYPOINT ["/usr/local/bin/sim-run", "serve", "--dir", "/srv"]
