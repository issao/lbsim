export function Home() {
  return (
    <div className="home">
      <h1>lbsim</h1>
      <p className="lede">
        A discrete-event simulator for a cloud LLM inference service. This is the stand-in dashboard: the layout, the
        tab structure and the pagination story, running against mock data generated in this browser so they can be
        criticised before anything is wired to a server.
      </p>

      <ul className="surfaces">
        <li>
          <a href="#/dashboard">Load test dashboard</a>
          <p>
            The fully customisable playground. Playback across the top, control panel left, observation suite right.
            Tune load and policy while it runs and watch what changes. <code>#/dashboard</code>
          </p>
        </li>
        <li>
          <a href="#/ab">A/B view</a>
          <p>
            The same load and the same seed, two policies side by side. Refuses to compare two runs that differ in
            anything but the policy, and says which field stopped it. <code>#/ab</code>
          </p>
        </li>
        <li>
          <a href="#/showcase">Showcase</a>
          <p>
            One card per interesting dynamic. Clicking one runs a scripted walkthrough that advances the run, pauses at
            the moments that matter and says what to look at. <code>#/showcase</code>
          </p>
        </li>
      </ul>

      <div className="footnote">
        <h2>What you are looking at</h2>
        <p>
          The engine does not exist yet, so every number here is invented by <code>web/src/lib/engine.ts</code> and
          every panel says so. The mock is arranged so the directions are right rather than the magnitudes: raising the
          arrival rate raises queue depth and then latency; round robin with a heavy tail produces a hotspot that walks
          the fleet; power-of-two-choices does not. That is enough to judge a layout and not enough to draw a
          conclusion about a policy.
        </p>
        <p>
          The shapes match <code>proto/lbsim/v1</code>: metrics are the <code>Metric</code> enum, distributions are
          bucketed histograms merged bucket-wise, a subscription names exactly one entity, and the responses that carry
          information the user is owed &mdash; where a step stopped, whether a rewind came from the recorded log,
          whether an update forced re-simulation &mdash; are surfaced rather than swallowed. See{' '}
          <code>web/README.md</code> for what has to change when <code>sim-ingress</code> exists.
        </p>
      </div>
    </div>
  );
}
