import { DATA_SOURCE_GLOSS } from '../lib/mode';

export function Home() {
  return (
    <div className="home">
      <h1>lbsim</h1>
      <p className="lede">
        lbsim is a discrete-event simulator of a cloud LLM inference fleet and the load balancer in front of it.
        Everything below is one of two kinds: live or replay.
      </p>

      <h2 className="section-label" style={{ fontSize: 13, fontWeight: 600, margin: '26px 0 6px' }}>
        Live
      </h2>
      <p className="lede">{DATA_SOURCE_GLOSS.live}.</p>
      <ul className="surfaces">
        <li>
          <a href="#/dashboard">Load test dashboard</a>
          <p>a run starts on the server when the page opens; tune load and policy while it runs</p>
        </li>
        <li>
          <a href="#/showcase">Showcase</a>
          <p>one walkthrough per dynamic; each drives a live run and pauses where it matters</p>
        </li>
        <li>
          <a href="#/ab">A/B view</a>
          <p>two live runs, same seed, different policy; a recorded pair when there is no server</p>
        </li>
      </ul>

      <h2 className="section-label" style={{ fontSize: 13, fontWeight: 600, margin: '26px 0 6px' }}>
        Replay
      </h2>
      <p className="lede">
        {DATA_SOURCE_GLOSS.replay}; the reports below are real runs rendered as HTML, reproducible from the scenario
        printed at the bottom of each.
      </p>
      <ul className="surfaces">
        <li>
          <a href="?server=off#/dashboard">Load test dashboard, replay</a>
          <p>the same dashboard over a recorded run; pick the run in the banner</p>
        </li>
        <li>
          <a href="reports/1-routing.html">1. Reading the whole fleet is worse than sampling two of it</a>
          <p>Four routing policies at identical load and seed.</p>
        </li>
        <li>
          <a href="reports/2-staleness.html">2. Herding is a smooth function of staleness, and it is steep</a>
          <p>Telemetry interval swept from 100 ms to 4 s.</p>
        </li>
        <li>
          <a href="reports/3-chunking.html">3. Prefill and decode contend for one device, and no setting wins both</a>
          <p>Chunked-prefill budget swept from 512 to 16,384 tokens.</p>
        </li>
        <li>
          <a href="reports/4-load-curve.html">4. Past the knee, offering more load delivers less work</a>
          <p>Offered load swept from 30 to 230 requests per second.</p>
        </li>
        <li>
          <a href="reports/5-long-context.html">
            5. Capacity is a token budget, and a load-balancer metric can improve while service collapses
          </a>
          <p>Long-context share swept from 0 to 32%.</p>
        </li>
        <li>
          <a href="reports/6-retry.html">6. A retry budget is the difference between a bad minute and an outage</a>
          <p>No retries, a retry budget, and a retry storm, after the same load spike.</p>
        </li>
        <li>
          <a href="reports/7-no-decode.html">7. The routing ordering does not need the decode physics</a>
          <p>Round robin against p2c with decode disabled (HBM to infinity).</p>
        </li>
        <li>
          <a href="reports/8-admission.html">8. Admission under overload: shed early or time out late</a>
          <p>Accept-all against deadline-aware admission.</p>
        </li>
        <li>
          <a href="reports/9-fair-share.html">9. Weighted fair share against an over-share tenant</a>
          <p>Tenants with accept-all against fair_share.</p>
        </li>
        <li>
          <a href="reports/10-probes.html">10. Live probes: least-KV on fresh state against p2c on the snapshot</a>
          <p>least_kv_tokens probing fresh state per request, against p2c reading a periodic snapshot.</p>
        </li>
        <li>
          <a href="reports/11-preemption.html">
            11. The KV spiral: parked session context fills the cache, and swapping it out cures it
          </a>
          <p>Preemption never against swap.</p>
        </li>
        <li>
          <a href="reports/12-spec-decode.html">12. Speculative decoding pays at small batch and costs at large</a>
          <p>N=0 against N=4 drafts.</p>
        </li>
        <li>
          <a href="reports/13-herd-fleet.html">13. Herding gets worse as the fleet grows</a>
          <p>least_requests over 32–512 replicas at the same offered/capacity ratio and 250 ms telemetry.</p>
        </li>
        <li>
          <a href="reports/14-affinity.html">14. Prefix affinity against load spreading</a>
          <p>p2c against prefix affinity at max load ratio 1.05 and 2.2, same prefix topology.</p>
        </li>
        <li>
          <a href="reports/15-gray-failure.html">15. Gray failure: a replica at 0.3x that announces healthy</a>
          <p>No detection against outlier ejection on the delayed view's step time.</p>
        </li>
        <li>
          <a href="reports/16-scheduling.html">16. The replica's own scheduler orders work, it does not create capacity</a>
          <p>fifo_chunked, class_priority and deadline_first over three SLO classes at 2x load: interactive attainment 0.926 to 0.943 and agent pays; deadline-first over chunked prefill is longest-job-first in disguise</p>
        </li>
        <li>
          <a href="reports/17-cascade.html">17. Affinity hotspot failover cascade: a crashed holder's prefixes land cold</a>
          <p>The hottest holder crashes at 60 s and returns cold at 90 s; p2c against affinity at max load ratio 2.2.</p>
        </li>
        <li>
          <a href="reports/18-bode.html">18. The staleness loop's Bode plot: gain, phase, and where it oscillates on its own</a>
          <p>A 30% sine on offered load swept from 0.01 to 1 Hz; least_requests on a 1 s / 200 ms delayed snapshot over 32 replicas.</p>
        </li>
      </ul>
      <p>
        Every number on every page is engine data. A quantity the engine does not simulate yet shows a dash and says
        so on hover.
      </p>
    </div>
  );
}
