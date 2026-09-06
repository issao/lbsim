# Brief template (the productivity agent owns this file; the tech lead pastes it verbatim)

Version 1, 2026-09-06 16:55 PDT, written from the first iteration profile. The tech lead fills the
bracketed parts; nothing else changes.

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
Work only in that worktree. Cargo only through `tools/build.sh`; inner loop is `tools/build.sh test -p
[crate]` or `--test [name]` (0.2–1.3 s); do not run the workspace tests or ./check-fingerprints.sh
yourself, integrate.sh does. Do not read tools/build.sh, check-fingerprints.sh, .cargo/config.toml or
tests/layering.rs: they do what this paragraph says. Send long command output to a file and `tail -20`
it; read files with offset/limit, never whole; your context is your speed.

Files owned: [exact paths]. Nothing else; if another file is needed, stop and say so in the report.
`git add` by explicit path; commit with `git commit -m "why, not what" -- <paths>`, ending with:
Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: [session url]

Read: [file:line ranges, with the relevant excerpts pasted here so the agent need not open them]

The unit: [what and why, the decisions already made, the stand-in interface if any]
Test first: [the test to write before the code]
Done when: [the observable: test name passes, fingerprint row added or unchanged, finding paragraph]

Exit ritual: commit, `git push -u origin claude/tl-[slug]`, then
`tools/integrate.sh claude/tl-[slug] --remove-worktree /home/agents/repo/lbsim-wt-[slug]`. If it exits
non-zero, do not retry more than once; report the stage and the output. Report in under 200 words: what
landed (hash), what you deliberately did not do, what the brief was missing.
---
