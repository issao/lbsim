# Walkthrough script format

A walkthrough is content, not interface. The server needs no knowledge of any of it: pausing uses
`SetSpeed(paused: true)` and `StepForward` with a bounded duration, both of which already exist in
`ingress.proto`, and the narration lives here.

```
{
  "id":       string,             unique; also the URL fragment (#/showcase/<id>)
  "dynamic":  number,             the row in docs/ARCHITECTURE.md section 12
  "title":    string,
  "summary":  string,
  "scenario": { <dotted path>: value },   overrides applied to the base scenario at start
  "steps": [
    {
      "at_sim_s":    number,      simulated seconds; the run advances to here, then pauses
      "speed":       number?,     realtime factor to use while advancing to this step
      "set":         { <dotted path>: value }?,   applied BEFORE advancing to this step
      "control_tab": "scenarios" | "load" | "policies" | "cluster" | "run",
      "observe_tab": "cluster" | "quality" | "machine" | "utilization" | "traces",
      "highlight":   string?,     the `id` of a panel, which gets an outline
      "title":       string,
      "body":        string[],    paragraphs
      "look_for":    string?      one line naming what to actually look at
    }
  ]
}
```

Two things are worth knowing about the semantics:

- **`set` applies on the way to a step, not on arrival.** A change that alters physics rewinds the
  run to its last snapshot and re-simulates, so a change applied on arrival would have no time to
  show its effect. Applying it before advancing means the effect has developed by the time the
  narration appears.
- **`at_sim_s` must increase.** The runner advances forward only; a step behind the cursor would
  require a rewind, which would discard the very history the previous step was describing.

`index.json` lists the cards. A card without a `script` is listed and disabled, so the catalogue
shows what exists and what does not rather than quietly omitting it.
