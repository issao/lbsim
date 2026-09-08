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
# run-demos.sh's Bode demo (18) post-processes with python3 bench/bode.py; the slim image has no
# python and .dockerignore keeps bench/ out except that one file. Exit 127 from the report step
# was this, the second time a 127 here pointed at nothing in the error.
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
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
# Generate the reports at build time, from the same script the repo runs locally, so the image
# cannot fall behind the demo list again (reports 11-16 each went missing here once, one by one).
# run-demos.sh drives cargo through tools/build.sh, whose flock lives under LBSIM_LOCK_DIR; the
# release binary is already built, so `cargo run --release` only links. Telemetry stays off: the
# image carries what a reader looks at, not a few megabytes of CSV nobody asked for.
# The report step keeps its own log in the image at /reports/build.log, because the deploy
# identity cannot read Cloud Build's logs and two builds in a row failed with a bare exit code.
# Until the deploy identity can read Cloud Build's log (roles/logging.viewer, TASKS.md), a failure
# here does not fail the build: the served log is the only way to read it.
RUN mkdir -p site/reports \
 && (LBSIM_LOCK_DIR=/tmp bash -x ./run-demos.sh > site/reports/build.log 2>&1; echo "run-demos exit $?" >> site/reports/build.log; true) \
 && (cp out/*.html site/reports/ 2>>site/reports/build.log || true) \
 && (ls -la out bench tools >> site/reports/build.log 2>&1; free -m >> site/reports/build.log 2>&1; nproc >> site/reports/build.log; true)
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
