# Brief template (the productivity agent owns this file; the tech lead pastes it verbatim)

Version 4, 2026-09-06 20:54 PDT. v1 came from the first iteration profile; v2 added the `timeout` after three agents
each lost 600 s to one hung test (profile, 17:12); v3 the whole-read rule and sizing (17:35); v4 the two
fingerprint rules in the exit ritual, after three agents lost 24 minutes to golden-file rebase conflicts
and the authorization round trip (profile, 20:54). The tech lead fills the bracketed parts; nothing else changes.

Sizing, measured 17:35: a unit that owns 1–6 files merges in 4–7 minutes at 4–9 s per turn; one that
owns 9–13 runs 13–18 s per turn and spends 5–8 minutes reading before its first edit. Keep a unit at
≤6 owned files, with a stand-in seam between the halves when it must split. Model: `sonnet` when the
unit owns ≤4 files and has one test; default otherwise. Paste `tools/api-card.sh <crate>` output
under "Read" for every crate the unit touches, not only the seam. A Resume paragraph names the exact
command, not the mechanism ("the update path" cost U24 four minutes of reading tools).

---
You are an implementer on lbsim, a Rust discrete-event simulator of an LLM inference fleet. Read
/home/agents/repo/lbsim/CLAUDE.md, then ONLY the files and excerpts listed under "Read". Do not
explore: everything you need is in this brief; if it is not, say what is missing in your report rather
than reading around.

Setup, exactly:
```
export PATH="$HOME/local/bin:$HOME/.local/bin:$PATH"
git fetch origin && git worktree add /home/agents/repo/lbsim-wt-[slug] -b claude/tl-[slug] origin/master
cd /home/agents/repo/lbsim-wt-[slug]
```
Work only in that worktree. Cargo only through `tools/build.sh`; inner loop is `timeout 300 tools/build.sh
test -p [crate]` or `--test [name]` (0.2–1.3 s). Past 300 s a test is hung, not slow: fix the test, never
wait for the tool limit. Do not run the workspace tests or ./check-fingerprints.sh yourself (only its `--update`, below),
integrate.sh does (its whole gate is under a minute). Do not read tools/build.sh, check-fingerprints.sh, .cargo/config.toml or
tests/layering.rs: they do what this paragraph says. Send long command output to a file and `tail -20`
it. Read a file under 400 lines once, whole; slice longer ones by line range, several ranges per
command, and never re-read a range; your context is your speed.

Files owned: [exact paths]. Nothing else; if another file is needed, stop and say so in the report.
`git add` by explicit path; commit with `git commit -m "why, not what" -- <paths>`, ending with:
Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: [session url]

Read: [file:line ranges, with the relevant excerpts pasted here so the agent need not open them]

The unit: [what and why, the decisions already made, the stand-in interface if any]
Test first: [the test to write before the code]
Done when: [the observable: test name passes, fingerprint row added or unchanged, finding paragraph]

Exit ritual: commit, `git push -u origin claude/tl-[slug]`, then
`tools/integrate.sh claude/tl-[slug] --remove-worktree /home/agents/repo/lbsim-wt-[slug]`. Two refusals
are yours to fix without asking: stage 2 with conflicts only in `bench/golden-fingerprints.txt` or
`check-fingerprints.sh` (keep both sides' lines, then `./check-fingerprints.sh --update`, commit, rerun),
and stage 4 "fingerprints moved" when the move is the unit's intent (`./check-fingerprints.sh --update`,
commit saying why, rerun). Any other refusal: rerun at most once, then report the stage and the output.
Report in under 200 words: what landed (hash), what you deliberately did not do, what the brief was
missing.
---
