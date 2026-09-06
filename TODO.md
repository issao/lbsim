# TODO

Tasks for the user. Claude keeps this current; anything Claude can do itself does not
belong here.

## Blocking — needs your action

- [ ] **Give this environment push access to GitHub.** All work is committed locally on
      `master` (5 commits ahead of `origin/master`) but the push failed: this sandbox has no
      GitHub credentials, and no `gh` CLI. Pick one:
      - Paste a personal access token with `repo` scope and I will configure
        `credential.helper store` for the remote. Simplest, and scoped to this repo.
      - Tell me to generate an SSH keypair here; you add the public key as a deploy key with
        write access on the repository, and I switch the remote to SSH.
      - Or pull these commits down yourself from wherever you can reach this filesystem.

## Blocking — implementation cannot start until these are done

- [ ] **Bless `docs/ARCHITECTURE.md`.** Six specific decisions are listed in its section 13.
      The load-bearing one is decision 1, analytic epoch advancement. If that is wrong,
      everything downstream changes.
- [ ] **Review the interfaces in `proto/`.** You asked to review every interface. Nine files.
      Suggested reading order, most consequential first:
      1. `policy.proto` — the engine/policy seam and the referee. Determines whether a
         policy can cheat.
      2. `telemetry.proto` — the staleness boundary. Determines whether the control-theory
         dynamics are reachable.
      3. `scenario.proto` — the whole configuration surface. Largest file, and the one you
         will live in.
      4. `common.proto`, `request.proto` — vocabulary. Note `Truth`, which holds what the
         simulator knows and no policy may see.
      5. `serving.proto`, `kv.proto`, `capacity.proto` — the modelled data plane, one method
         per diagram arrow.
      6. `metrics.proto`, `control.proto` — results and the dashboard API.

## Waiting on you, not blocking

- [x] ~~Draw the wire diagram~~ Claude drew `docs/diagrams/system.drawio` instead, two
      pages, uncompressed. Open it in draw.io to read or edit. `tools/check_diagram.py`
      verifies every arrow against `proto/` and runs clean.
- [x] ~~Fill VISION section 9~~ Points at `docs/ARCHITECTURE.md` section 10, which describes
      the system in prose.
- [ ] Decide whether the dashboard is scoped as a separate workstream. It is a substantial
      product on its own and will expand without limit if it shares a backlog with the
      engine. See `docs/ARCHITECTURE.md` risk 7.

## Questions Claude needs answered eventually

- [ ] Do you have access to real production traces for calibration, even in aggregate form?
      Workload realism is the weakest link and the least glamorous thing to get right.
      See `docs/ARCHITECTURE.md` risk 6.
- [ ] Reference hardware and model for calibration. The primer assumes a 70B-class model on
      8x H100. Confirm, or name the pair you want as the default.
- [ ] Target for the agent arena: which metric should an agent optimise? Goodput alone is
      gameable by starving batch traffic, so it likely needs a fairness or SLO-attainment
      constraint alongside it.

## Done

- [x] Write `VISION.md` sections 1-8.
- [x] Publish the repository to GitHub.
