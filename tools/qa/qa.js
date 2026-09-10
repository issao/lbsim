// Browser gate for the dashboard. Drives a real Chromium against a running server (local
// `sim-run serve` or lbsim.ai) and asserts the target state of the product, not today's: a check
// that fails here is a unit of work someone owes, which is why every check names what it saw.
//
// Usage: QA_BASE=http://localhost:8181 node tools/qa/qa.js   (tools/qa/serve-local.sh wraps this)
const { chromium } = require('playwright-core');
const fs = require('fs');

const BASE = process.env.QA_BASE || 'http://localhost:8181';
const MIN_CARDS = 19;
// The number of demo reports is whatever run-demos.sh produces, read from the script itself so a new
// demo never fails this check by being new; it failed once at 21 with a hard-coded 20.
const EXPECTED_REPORTS = (() => {
  try {
    const src = require('fs').readFileSync(require('path').join(__dirname, '..', '..', 'run-demos.sh'), 'utf8');
    const n = new Set([...src.matchAll(/--out out\/(\d+)-[a-z0-9-]+\.html/g)].map((x) => x[1])).size;
    return n || 20;
  } catch { return 20; }
})();
// Cards whose scenario overrides are the whole point of the demo; a walkthrough that runs
// without them looks fine and shows nothing.
const REQUIRED_KEYS = {
  'KV preemption spiral at low load': [/preemption\s*=\s*never\b/, /session_turns_mean\s*=\s*8\b/],
  'Speculative decoding value and cost': [/spec_draft_tokens\s*=\s*4\b/],
  // U113: SCENARIO_KEYS lacked the prefix-topology keys and ROUTING_TO_ENGINE nulled
  // prefix_affinity, so this card's live run silently ran p2c without a prefix model at all.
  'Prefix affinity against load spreading': [/prefix_roots\s*=\s*100\b/, /routing\s*=\s*prefix_affinity\b/],
};
const CHROME = [
  '/home/agents/.cache/ms-playwright/chromium-1140/chrome-linux/chrome',
  '/home/agents/.cache/ms-playwright/chromium_headless_shell-1234/chrome-linux/headless_shell',
  '/home/agents/.cache/ms-playwright/chromium-1234/chrome-linux/chrome',
].find(p => fs.existsSync(p));

const results = [];
const check = (name, ok, detail = '') => {
  results.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${name}${detail ? ' — ' + String(detail).slice(0, 220) : ''}`);
  return ok;
};
const sleep = ms => new Promise(r => setTimeout(r, ms));

// A hung browser must still produce a verdict: the whole run is capped so a stuck page fails
// loudly instead of waiting on whoever invoked us.
setTimeout(() => { console.log('FAIL harness — global timeout'); console.log(finalLine(1)); process.exit(1); }, 15 * 60 * 1000).unref();
const finalLine = extraFail => {
  const failed = results.filter(r => !r).length + extraFail;
  return `qa: ${results.filter(Boolean).length} passed, ${failed} failed`;
};

(async () => {
  if (!CHROME) throw new Error('no Chromium under ~/.cache/ms-playwright');
  const browser = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });

  // Every run the harness starts is its own to stop: the server caps live runs at 8 and answers
  // 503 past it, so a harness that leaked its runs failed every card from the ninth on.
  const started = new Set();
  const stopped = new Set();
  const stopRuns = async () => {
    for (const id of started) {
      if (stopped.has(id)) continue;
      stopped.add(id);
      await ctx.request.post(BASE + '/v1/ingress/StopRun', { data: { run_id: id } }).catch(() => null);
    }
  };

  // Every load gets a fresh page so it is a real navigation and its event log starts empty.
  const fresh = async (hash) => {
    const page = await ctx.newPage();
    const log = { errs: [], bad: [], startRuns: [], updatePolicies: [], requests: [] };
    page.on('pageerror', e => log.errs.push(e.message.slice(0, 200)));
    page.on('console', m => { if (m.type() === 'error' && !m.text().includes('SetSpeed')) log.errs.push('console: ' + m.text().slice(0, 200)); });
    page.on('response', r => {
      if (r.status() >= 400 && !r.url().includes('SetSpeed')) log.bad.push(`${r.status()} ${r.url().slice(0, 100)}`);
      if (r.url().endsWith('/StartRun') && r.status() === 200) r.json().then(j => { if (j && j.run_id) started.add(j.run_id); }).catch(() => null);
    });
    page.on('request', r => {
      log.requests.push(r.url());
      if (r.url().endsWith('/StartRun') && r.postData()) log.startRuns.push(r.postData());
      // wr_c1..c5 (weighted_random) and scheduling/buffer_max_* (buffered_batch) go by two
      // different paths -- UpdatePolicies live, StartRun on restart -- so the harness needs both bodies.
      if (r.url().endsWith('/UpdatePolicies') && r.postData()) log.updatePolicies.push(r.postData());
    });
    await page.goto(BASE + '/' + hash, { waitUntil: 'domcontentloaded', timeout: 30000 });
    const body = async () => (await page.evaluate(() => document.body.innerText)).replace(/\s+/g, ' ');
    // Poll rather than sleep so a healthy page moves on quickly and a sick one still gets its full grace.
    const until = async (pred, ms, step = 250) => {
      const end = Date.now() + ms;
      for (;;) { const v = await pred(); if (v) return v; if (Date.now() > end) return v; await sleep(step); }
    };
    return { page, log, body, until };
  };
  const cardCount = page => page.$$eval('button.card', els => els.length);
  const hasWt = page => page.$('.wt-mode').then(Boolean);
  const badge = page => page.$eval('.mode-badge', el => el.textContent.trim()).catch(() => null);
  // The Machine level tab is the only place per-replica rows appear, and on every source it once
  // said "0 replicas exist" (Issao: "the machine page always shows 0 replicas"). The pager count and
  // the table must agree that at least one replica is there.
  const machines = async (page, until, label) => {
    // Waited for, not looked for once: on a replay page the tab strip follows the recording's first
    // frames, and a one-shot lookup that lost the race read as "no Machine level tab".
    const tab = await until(() => page.$('button[data-tab="observe:machine"]'), 4000);
    if (tab) await tab.click();
    const seen = await until(async () => {
      const pager = await page.$eval('#replicas .pager .grow', el => el.textContent).catch(() => '');
      const m = /(\d+) replicas exist/.exec(pager || '');
      const rows = await page.$$eval('#replicas tbody tr:not([aria-hidden])', els => els.length).catch(() => 0);
      // A row whose queue cell reads "—" exists but has not streamed; live rows arrive one
      // subscription each, so at least one must have a number before this counts as fixed.
      const streamed = await page.$$eval('#replicas tbody td.bar-cell span', els => els.filter(e => e.textContent.trim() !== '—').length).catch(() => 0);
      return m && Number(m[1]) > 0 && rows >= 1 && streamed >= 1 ? { exist: Number(m[1]), rows, streamed } : null;
    }, 8000);
    check(`${label}: machines shows replicas (U90)`, Boolean(seen),
      seen ? `${seen.exist} exist, ${seen.rows} rows on the page, ${seen.streamed} with values` : tab ? 'pager says 0 replicas exist, or no row has streamed a value' : 'no Machine level tab');
  };

  // U95b (Issao: "what is mock about a live showcase run?"), then U100 (Issao: "remove all invented
  // numbers everywhere"): no panel may show a number the engine did not produce, and nothing on the
  // page may call itself mock. Walk every observation tab, since a panel only mounts once its tab
  // is selected: no per-panel tag or partial note may exist at all, the tab bodies the engine does
  // not feed yet (traces) must say so in words, and on the Machines tab the one column the engine
  // does not produce (prefix hit rate, REPLICA_COLUMNS index 6) must read "—" on every row, while
  // the state column (index 1), wired by U101, must carry a real state on at least one row.
  const noInvented = async (page, label) => {
    const obsTabs = await page.$$eval('button[data-tab^="observe:"]', els => els.map(e => e.getAttribute('data-tab')));
    const bad = obsTabs.length === 0 ? ['no observation tabs'] : [];
    for (const tabId of obsTabs) {
      await page.click(`button[data-tab="${tabId}"]`);
      await sleep(500);
      const tags = await page.$$eval('.mock-tag, .partial-note', els => els.length);
      if (tags) bad.push(`${tabId}: ${tags} mock tag(s) or partial note(s)`);
      const words = await page.$$eval('.panel, .note', els => els.map(e => e.textContent).filter(t => /\bmock\b/i.test(t)).length);
      if (words) bad.push(`${tabId}: ${words} element(s) say mock`);
      if (tabId === 'observe:traces') {
        // U104: the tab lists the engine's own sampled journeys. Live, the first page arrives with
        // the first completions; on a recording, traces.jsonl is one fetch. A recording exported
        // before traces existed says so in words, which is not an invented number.
        const end = Date.now() + 10000;
        let rows = 0, t = '';
        for (;;) {
          rows = await page.$$eval('#trace-list tbody tr', trs => trs.length).catch(() => 0);
          t = await page.$eval('#trace-list', el => el.textContent.trim()).catch(() => '');
          if (rows > 0 || /carries no traces/.test(t) || Date.now() > end) break;
          await sleep(250);
        }
        if (rows === 0 && !/carries no traces/.test(t)) bad.push(`traces: no sampled request listed after 10 s; panel says ${JSON.stringify(t.slice(0, 80))}`);
      }
      if (tabId === 'observe:machine') {
        const rows = await page.$$eval('#replicas tbody tr:not([aria-hidden])', trs => trs.map(tr => [...tr.querySelectorAll('td')].map(td => td.textContent.trim())));
        if (rows.length === 0) bad.push('machine: no rows');
        const prefixValued = rows.filter(r => r[6] !== '—').length;
        if (prefixValued) bad.push(`machine: ${prefixValued} of ${rows.length} rows carry a value in the unwired prefix-hit column`);
        const stated = rows.filter(r => /^(ready|degraded|ejected)\b/.test(r[1])).length;
        if (stated === 0) bad.push(`machine: no row carries a real state (U101); first row ${JSON.stringify(rows[0] || [])}`);
      }
    }
    check(label, bad.length === 0, bad.length ? bad.join(' · ') : `${obsTabs.length} tabs clean`);
  };

  // a. home
  {
    const { page, log, body, until } = await fresh('#/');
    await until(() => page.$$eval('a[href^="reports/"]', a => a.length), 5000);
    const t = await body();
    check('home loads', t.length > 200, t.slice(0, 100));
    const hrefs = [...new Set(await page.$$eval('a[href^="reports/"]', as => as.map(a => a.getAttribute('href'))))];
    for (const h of hrefs) {
      const r = await page.request.get(BASE + '/' + h);
      check(`home link ${h}`, r.status() === 200, String(r.status()));
    }
    check('home: every demo report linked', hrefs.length === EXPECTED_REPORTS, `${hrefs.length} links, run-demos.sh has ${EXPECTED_REPORTS}`);
    check('home: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    const homeBadge = await badge(page);
    check('home: badge empty', homeBadge === '', homeBadge);
    const sections = await page.$$eval('h2', els => els.map(e => e.textContent.trim()));
    check('home: two sections', JSON.stringify(sections) === JSON.stringify(['Live', 'Replay']), sections.join(', '));
    const mockLinks = await page.$$eval('a', as => as.map(a => a.getAttribute('href') || '').filter(h => /replay=off|replay=0/.test(h)));
    check('home: no mock links', mockLinks.length === 0, mockLinks.join(' '));
    await page.close();
  }

  // b. live dashboard
  {
    const { page, log, body, until } = await fresh('#/dashboard');
    const ready = t => !/waiting for the first sample/i.test(t) && /\blive\b/i.test(t) && /run r-\d+/.test(t);
    let t = await until(async () => { const b = await body(); return ready(b) ? b : null; }, 12000) || await body();
    check('dashboard renders samples', !/waiting for the first sample/i.test(t), t.slice(0, 160));
    check('dashboard says live', /\blive\b/i.test(t) && /run r-\d+/.test(t), (t.match(/run r-\d+[^|]{0,60}/) || [''])[0]);
    const dashboardBadge = await badge(page);
    check('dashboard: badge live with run id', /^live — .*· run r-\d+$/.test(dashboardBadge || ''), dashboardBadge);
    const banner = (t.match(/run r-\d+.{0,120}/) || [''])[0];
    check('dashboard banner not speed 0×', !/speed 0×/.test(t), banner.slice(0, 120));
    const slider = async () => page.$eval('[role=slider]', el => ({
      now: Number(el.getAttribute('aria-valuenow')), max: Number(el.getAttribute('aria-valuemax')),
    })).catch(() => null);
    const s1 = await slider();
    await sleep(4000);
    const s2 = await slider();
    const grew = s1 && s2 && s2.now - s1.now >= 2;
    const paced = s2 && (!Number.isFinite(s2.max) || s2.max === 0 || s2.now < s2.max);
    check('dashboard advances at 1x', Boolean(grew && paced),
      s1 && s2 ? `${s1.now} -> ${s2.now} over 4 s (max ${s2.max})` : 'no [role=slider]');
    check('dashboard: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    check('dashboard: no 4xx/5xx', log.bad.length === 0, log.bad.slice(0, 3).join(' | '));
    const tags = await page.$$eval('.mock-tag', els => els.length);
    check('dashboard: no per-panel tags (U100)', tags === 0, `${tags} .mock-tag`);
    await machines(page, until, 'dashboard');
    // U104: the Traces tab lists journeys the engine sampled, and every replica it names is one
    // the fleet has. The ready count is the Cluster tab's tile, the fleet row's own number.
    {
      await page.click('button[data-tab="observe:cluster"]');
      const ready = await until(async () => {
        const v = await page.$$eval('.tile', els => {
          const t = els.find(e => (e.querySelector('.tile-label') || {}).textContent?.trim() === 'ready');
          return t ? t.querySelector('.tile-value')?.textContent?.trim() : null;
        }).catch(() => null);
        return v && /^\d+$/.test(v) ? Number(v) : null;
      }, 5000);
      await page.click('button[data-tab="observe:traces"]');
      const seen = await until(async () => {
        const rows = await page.$$eval('#trace-list tbody tr', trs => trs.length).catch(() => 0);
        return rows > 0 ? rows : null;
      }, 10000);
      const ids = await page.$$eval('#trace-list [data-replica]', els => els.map(e => Number(e.getAttribute('data-replica')))).catch(() => []);
      const strays = ready === null ? ids : ids.filter(id => !(Number.isInteger(id) && id >= 0 && id < ready));
      check('traces: live run lists sampled requests with real replica ids (U104)', Boolean(seen) && ready !== null && ready > 0 && strays.length === 0,
        seen ? `${seen} rows, ${ids.length} replica ids shown, ${ready} ready${strays.length ? `, strays ${strays.slice(0, 5).join(',')}` : ''}` : ready === null ? 'no ready tile on the Cluster tab' : 'no trace row within 10 s');

      // Issao: "for the traces view, show the table with sampled requests at the top." The table
      // renders above the detail (never below or beside it), and clicking a row draws that
      // request's span timeline underneath.
      if (seen) {
        const box = async sel => page.$eval(sel, el => el.getBoundingClientRect()).catch(() => null);
        const tableBox = await box('#trace-list table.data');
        const rowCount = await page.$$eval('#trace-list tbody tr', trs => trs.length).catch(() => 0);
        // Row 2 if there is one, so the detail is proven to follow the click rather than showing
        // whichever row was already selected by default.
        await page.click(`#trace-list tbody tr:nth-child(${Math.min(2, rowCount)})`).catch(() => null);
        const detailBox = await box('#trace-list .trace-detail');
        const spans = await page.$$eval('#trace-list .trace-detail svg', els => els.length).catch(() => 0);
        check('traces: the sample table renders above the span timeline, not below or beside it',
          Boolean(tableBox && detailBox && tableBox.top < detailBox.top && spans > 0),
          tableBox && detailBox
            ? `table top ${tableBox.top.toFixed(0)}, detail top ${detailBox.top.toFixed(0)}, ${spans} span timeline(s)`
            : `table ${JSON.stringify(tableBox)}, detail ${JSON.stringify(detailBox)}`);

        // Issao: "The traces seems to show bars with a point in time. They should show width
        // proportional of each trace duration (so e.g. if a request was in a queue for a while, the
        // bar for queue should be wide indicating start and stop)." Read the decoded duration and
        // rendered width straight off the DOM data attributes `Waterfall.tsx` sets (`data-duration-ms`,
        // `data-width-px`), rather than trusting a screenshot, and find a pair of spans whose
        // durations differ by at least 3x to prove the width follows.
        const bars = await page.$$eval('#trace-list .trace-detail .wf-bar', els => els.map(e => ({
          durationMs: Number(e.getAttribute('data-duration-ms')),
          widthPx: Number(e.getAttribute('data-width-px')),
          op: e.getAttribute('data-op'),
          component: e.getAttribute('data-component'),
        })));
        const findWidePair = () => {
          for (const a of bars) for (const b of bars) if (a.durationMs > 0 && b.durationMs / a.durationMs >= 3) return [a, b];
          return null;
        };
        const widePair = findWidePair();
        check('traces: a span bar at least 3x longer in duration is at least 3x wider, not a point',
          Boolean(widePair) && widePair[1].widthPx / widePair[0].widthPx >= 3,
          widePair
            ? `${widePair[0].op} ${widePair[0].durationMs.toFixed(2)}ms = ${widePair[0].widthPx.toFixed(1)}px vs ${widePair[1].op} ${widePair[1].durationMs.toFixed(2)}ms = ${widePair[1].widthPx.toFixed(1)}px`
            : `no pair of the ${bars.length} span bars differs by >=3x in duration on this trace`);

        // Hovering a bar must show the machine (component/replica id) and both the start and stop
        // times, per Issao: "a mouse over to a bar in the trace should show machine id, start and
        // stop time."
        if (bars.length > 0) {
          const target = await page.$('#trace-list .trace-detail .wf-bar');
          await target?.hover().catch(() => null);
          const tip = await until(async () =>
            page.$eval('#trace-list .trace-detail .tooltip', el => el.textContent).catch(() => null), 2000);
          check('traces: hovering a span bar shows the machine id and both start/stop times',
            Boolean(tip) && /machine/i.test(tip) && /start/i.test(tip) && /stop/i.test(tip) && bars[0].component && tip.includes(bars[0].component),
            tip ? tip.replace(/\s+/g, ' ').slice(0, 200) : 'no tooltip appeared on hover');
        }

        // Issao: "in the traces, please show prompt and output size for each request." Both are
        // sortable columns like the latency ones, exact counts (thousands-separated, so strip
        // commas before checking the cell is an integer).
        const headers = await page.$$eval('#trace-list table.data thead th', els => els.map(e => e.textContent.trim().toLowerCase()));
        const promptIdx = headers.findIndex(h => h.startsWith('prompt'));
        const outputIdx = headers.findIndex(h => h.startsWith('output'));
        check('traces: table header carries prompt and output columns',
          promptIdx >= 0 && outputIdx >= 0, headers.join(', '));
        if (promptIdx >= 0 && outputIdx >= 0) {
          const firstRow = await page.$$eval('#trace-list tbody tr:first-child td', els => els.map(e => e.textContent.trim()));
          const isIntCell = s => s === '—' || /^\d{1,3}(,\d{3})*$/.test(s);
          check('traces: the first row\'s prompt and output cells are integers (or "—")',
            isIntCell(firstRow[promptIdx]) && isIntCell(firstRow[outputIdx]),
            `prompt=${JSON.stringify(firstRow[promptIdx])} output=${JSON.stringify(firstRow[outputIdx])}`);
        }
      }
    }
    // Issao: "looking at traces, I see out of SLO request where there is a very short queue (31ms)
    // then a big gap (1.6s) until the first prefill. There shouldn't be a big gap in the trace view
    // with unaccounted time." Read the engine's own spans over the wire for the first ten sampled
    // journeys of this run and demand that consecutive spans leave no gap over 100 ms: a sequence
    // admitted to a batch and waiting for the prefill budget is now a `prefill_wait` span, not
    // silence. A journey that never reached a replica (rejected, timed out queued) has nothing to
    // be contiguous with and is skipped.
    {
      const runId = ((dashboardBadge || '').match(/run (r-\d+)/) || [])[1];
      const finished = new Set(['OUTCOME_OK', 'OUTCOME_OK_SLO_VIOLATED']);
      let traces = [];
      if (runId) {
        // Traces exist only once requests complete; on the low-rps kv-spiral fleet the first completions
        // land ~20 simulated seconds in, and a walkthrough pauses at its first narration point (12 s), so
        // press the card's Play and let the run reach 30 simulated seconds before reading.
        const playBtn = await page.$('.btn.wt-play').catch(() => null);
        if (playBtn) await playBtn.click().catch(() => undefined);
        for (let attempt = 0; attempt < 30; attempt++) {
          const g = await ctx.request.post(BASE + '/v1/ingress/GetRun', { data: { run_id: runId } }).catch(() => null);
          const j = g && g.ok() ? await g.json().catch(() => ({})) : {};
          const simS = j.sim_time_unix_ns ? Number((BigInt(j.sim_time_unix_ns) - 1767225600000000000n) / 1000000000n) : 0;
          if (simS >= 30 || j.state === 'STATE_COMPLETE') break;
          if (j.state === 'STATE_PAUSED' && playBtn) await playBtn.click().catch(() => undefined);
          await sleep(2000);
        }
        for (let attempt = 0; attempt < 15 && traces.length === 0; attempt++) {
          const r = await ctx.request.post(BASE + '/v1/ingress/GetTraces', { data: { run_id: runId, limit: 10 } }).catch(() => null);
          traces = r && r.ok() ? ((await r.json().catch(() => ({}))).traces || []) : [];
          if (traces.length === 0) await sleep(3000);
        }
      }
      const gaps = [];
      let waits = 0;
      for (const t of traces) {
        if (!finished.has(t.record && t.record.outcome)) continue;
        const spans = (t.spans || []).map(s => ({ op: s.operation, start: BigInt(s.start_unix_ns), end: BigInt(s.end_unix_ns) }));
        for (let i = 1; i < spans.length; i++) {
          const gapMs = Number(spans[i].start - spans[i - 1].end) / 1e6;
          if (gapMs > 100) gaps.push(`${t.record.id}: ${gapMs.toFixed(0)} ms between ${spans[i - 1].op} and ${spans[i].op}`);
          if (spans[i].op === 'prefill_wait') waits++;
        }
      }
      check('traces: no gap over 100 ms between consecutive spans of the first ten sampled journeys (Issao)',
        Boolean(runId) && traces.length > 0 && gaps.length === 0,
        !runId ? 'no run id on the badge' : traces.length === 0 ? 'GetTraces returned no traces' : gaps.length ? gaps.slice(0, 3).join(' · ') : `${traces.length} traces, ${waits} prefill_wait spans, contiguous`);
    }
    await noInvented(page, 'dashboard: no invented numbers on live (U95b)');

    // Smoothing (Issao: "a global selector of a window average to be applied on them, live
    // selectable ... ideally it is a metric subscription that we pass down to the leaves"). On a
    // live run choosing 30 s reopens the fleet subscription with `smoothing_window_ns` on the query,
    // and the arrivals chart's series vary less than they did per sample. The offered rate itself
    // is a constant on the default scenario (no perturbation), so the variance is read off the
    // completed series of the same chart, the noisy one; the offered series must not grow noisier.
    //
    // A trailing mean over a *complete* window cannot raise a stationary series' variance, but
    // WIRE.md says outright that "a run's first seconds smooth over the frames that exist rather
    // than wait for a full window" -- by design, not a bug. `framesInWindow` (adapter.ts) is
    // `ceil(30s / sample_interval)`; at this scenario's 4 samples/sim s that is 120 frames, so every
    // one of the first ~40 points this check used to compare is a partial window, barely smoothed,
    // and comparing variance there is comparing noise to noise. Confirmed by hand against lbsim.ai
    // (playwright, collecting ~150 points): over the first 120 points variance barely moves
    // (13493.82 -> 13307.53), while over the points *past* the first full window it drops by an
    // order of magnitude (2220.82 -> 246.64), exactly as the trailing-mean math promises. So this
    // check now waits past the first full window and compares only the points beyond it, on both
    // series at the same indices.
    {
      await page.click('button[data-tab="observe:cluster"]').catch(() => null);
      const values = async key => page.$eval(`#arrivals path[data-key="${key}"]`, el => el.getAttribute('data-values'))
        .then(v => v.split(',').map(Number).filter(Number.isFinite)).catch(() => []);
      const variance = xs => { const m = xs.reduce((a, b) => a + b, 0) / xs.length; return xs.reduce((a, x) => a + (x - m) * (x - m), 0) / xs.length; };
      // The banner states the run's own sample rate ("N samples at R/sim s"); read it rather than
      // assuming BASE's, so this keeps working if the default scenario's rate ever changes.
      const rateMatch = /at ([\d.]+)\/sim s/.exec(await body());
      const rate = rateMatch ? Number(rateMatch[1]) : 4;
      const WINDOW_S = 30;
      const framesInWindow = Math.max(1, Math.ceil(WINDOW_S * rate));
      const TAIL = 60; // points compared once every one of them has a complete trailing window
      const need = framesInWindow + TAIL;
      await page.click('.playback [data-smooth="off"]').catch(() => null);
      const rawAll = await until(async () => { const v = await values('completed'); return v.length >= need ? v : null; }, 90000) || [];
      const rawOffered = await values('offered');
      const before = log.requests.length;
      await page.click('.playback [data-smooth="30s"]');
      const reopened = await until(() => log.requests.slice(before).find(u => u.includes('OpenSubscription') && u.includes('smoothing_window_ns=30000000000')) || null, 5000);
      check('smoothing: 30 s reopens the fleet subscription with smoothing_window_ns (Issao)', Boolean(reopened),
        reopened ? reopened.slice(reopened.indexOf('?'), reopened.indexOf('?') + 200) : `no OpenSubscription with smoothing_window_ns=30000000000 among ${log.requests.length - before} requests`);
      // The smoothed history arrives as one swap once the reopened stream has caught up.
      const rawJoin = rawAll.join();
      const smoothAll = await until(async () => {
        const v = await values('completed');
        return v.length >= rawAll.length && v.join() !== rawJoin ? v : null;
      }, 60000) || [];
      const smoothOffered = await values('offered');
      const n = Math.min(rawAll.length, smoothAll.length);
      const rawTail = rawAll.slice(framesInWindow, n);
      const smoothTail = smoothAll.slice(framesInWindow, n);
      const vr = variance(rawTail), vs = variance(smoothTail);
      check('smoothing: the arrivals chart varies less at 30 s than per sample', n >= need && rawTail.length >= TAIL && vs < vr && variance(smoothOffered) <= variance(rawOffered) + 1e-9,
        `completed: variance ${vr.toFixed(2)} per sample -> ${vs.toFixed(2)} at 30 s over ${rawTail.length} points past the first ${framesInWindow}-frame window (of ${n}/${rawAll.length}/${smoothAll.length} raw/collected/smoothed); offered ${variance(rawOffered).toFixed(3)} -> ${variance(smoothOffered).toFixed(3)}`);
      check('smoothing: the URL carries the selection', /[?&]smooth=30s/.test(page.url()), page.url().slice(0, 120));
      const readout = await page.$eval('#arrivals .panel-sub', el => el.textContent).catch(() => '');
      check('smoothing: the chart readout states the window', /30 s window/.test(readout), readout.slice(0, 80));
      // The selection is remembered per browser; put it back so the pages that follow open raw.
      await page.click('.playback [data-smooth="off"]');
      await until(async () => !(await page.$eval('.playback [data-smooth="30s"]', el => el.getAttribute('aria-pressed') === 'true')), 2000);
    }

    // U106 (Issao: "where do i tune step token budget?"): the Cluster tab's physics knobs are
    // editable on a live run. A structural edit is staged, the banner asks for a restart naming
    // the key, and the restart starts a run whose scenario text carries the new value.
    {
      const runIdOf = t => (/run (r-\d+)/.exec(t) || [])[1];
      const before = runIdOf(await body());
      await page.click('button[data-tab="control:cluster"]').catch(() => null);
      await sleep(300);
      // React owns the input's value, so the native setter plus an input event is what a drag is.
      const moved = await page.evaluate(() => {
        const lab = [...document.querySelectorAll('label.field')].find(l => (l.querySelector('.field-label')?.textContent || '').trim() === 'step token budget');
        const el = lab && lab.querySelector('input[type=range]');
        if (!el) return null;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, '2048');
        el.dispatchEvent(new Event('input', { bubbles: true }));
        return el.value;
      });
      const staged = await until(async () => { const b = await body(); return /restart the run to apply/i.test(b) ? b : null; }, 3000) || await body();
      check('cluster: structural change shows the restart banner (U106)', /restart the run to apply.*step token budget/i.test(staged),
        moved === null ? 'no step token budget slider on the Cluster tab' : (staged.match(/restart the run to apply[^|]{0,80}/) || [`slider at ${moved}, no banner`])[0]);
      const startRunsBefore = log.startRuns.length;
      await page.click('#restart-pending').catch(() => null);
      const after = await until(async () => { const id = runIdOf(await body()); return id && id !== before ? id : null; }, 15000);
      const bodies = log.startRuns.slice(startRunsBefore).map(b => { try { return JSON.parse(b)?.scenario?.text ?? b; } catch { return b; } });
      const applied = bodies.some(b => /^step_token_budget = 2048$/m.test(b));
      check('cluster: restart applies the new value (U106)', Boolean(after) && applied,
        `${before} -> ${after || 'no new run'}; ${bodies.length} StartRun(s), ${applied ? 'step_token_budget = 2048 sent' : (bodies[bodies.length - 1] || 'no scenario text').match(/step_token_budget[^\n]*/)?.[0] || 'no step_token_budget'}`);
    }

    // Issao: "the buffer batch scheduler and the weighted random router (with ability to tune in
    // ui)". Routing and its coefficients are live (UpdatePolicies, forward-only); the scheduler and
    // its buffer are physics-class, staged for a restart like the Cluster tab's knobs above.
    {
      const selectByLabel = (label, value) => page.evaluate(({ label, value }) => {
        const lab = [...document.querySelectorAll('label.field')].find(l => (l.querySelector('.field-label')?.textContent || '').trim() === label);
        const el = lab && lab.querySelector('select');
        if (!el) return false;
        const set = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value').set;
        set.call(el, value);
        el.dispatchEvent(new Event('change', { bubbles: true }));
        return el.value === value;
      }, { label, value });
      const setSlider = (label, value) => page.evaluate(({ label, value }) => {
        const lab = [...document.querySelectorAll('label.field')].find(l => (l.querySelector('.field-label')?.textContent || '').trim() === label);
        const el = lab && lab.querySelector('input[type=range]');
        if (!el) return null;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, String(value));
        el.dispatchEvent(new Event('input', { bubbles: true }));
        return el.value;
      }, { label, value });
      const lastOverrides = body => { try { return JSON.parse(body)?.overrides ?? null; } catch { return null; } };

      await page.click('button[data-tab="control:policies"]').catch(() => null);
      await sleep(300);

      // weighted_random: routing.kind and its five coefficients apply live.
      const wrSelected = await selectByLabel('policy', 'weighted_random');
      const hasC1 = await until(() => page.evaluate(() =>
        [...document.querySelectorAll('label.field .field-label')].some(el => el.textContent.trim() === 'c1')), 2000);
      const c2Before = log.updatePolicies.length;
      const c2Moved = await setSlider('c2', -0.5);
      const c2Sent = await until(() => {
        for (let i = log.updatePolicies.length - 1; i >= c2Before; i--) {
          const o = lastOverrides(log.updatePolicies[i]);
          if (o && o.wr_c2 !== undefined) return o;
        }
        return null;
      }, 4000);
      check('policies: weighted_random is selectable', wrSelected, wrSelected ? 'selected' : 'no <select> labeled "policy", or option missing');
      check('policies: moving c2 sends UpdatePolicies with wr_c2 = -0.5 (live, forward-only)',
        Boolean(c2Sent) && c2Sent.wr_c2 === '-0.5',
        c2Moved === null ? 'no c2 slider on the Policies tab' : c2Sent ? `overrides ${JSON.stringify(c2Sent)}` : `${log.updatePolicies.length - c2Before} UpdatePolicies since selecting weighted_random, none carrying wr_c2`);

      // buffered_batch: the scheduler and its buffer stage a restart instead.
      const runIdOf = t => (/run (r-\d+)/.exec(t) || [])[1];
      const beforeSched = runIdOf(await body());
      const schedSelected = await selectByLabel('scheduler', 'buffered_batch');
      const holdMoved = await until(() => setSlider('buffer max hold', 10), 2000);
      const stagedSched = await until(async () => { const b = await body(); return /restart the run to apply/i.test(b) ? b : null; }, 3000) || await body();
      check('policies: buffered_batch scheduler is selectable', schedSelected, schedSelected ? 'selected' : 'no <select> labeled "scheduler", or option missing');
      check('policies: the buffer knobs appear once buffered_batch is selected, and hold is settable',
        holdMoved !== null && holdMoved !== '5', holdMoved === null ? 'no "buffer max hold" slider appeared' : `moved to ${holdMoved}`);
      check('policies: scheduling change shows the restart banner (buffer max hold)', /restart the run to apply.*buffer max hold/i.test(stagedSched),
        (stagedSched.match(/restart the run to apply[^|]{0,120}/) || ['no banner'])[0]);
      const startRunsBeforeSched = log.startRuns.length;
      await page.click('#restart-pending').catch(() => null);
      const afterSched = await until(async () => { const id = runIdOf(await body()); return id && id !== beforeSched ? id : null; }, 15000);
      const schedBodies = log.startRuns.slice(startRunsBeforeSched).map(b => { try { return JSON.parse(b)?.scenario?.text ?? b; } catch { return b; } });
      const schedApplied = schedBodies.some(b => /^scheduling = buffered_batch$/m.test(b) && /^buffer_max_hold_ms = 10$/m.test(b));
      check('policies: restart applies scheduling = buffered_batch, buffer_max_hold_ms = 10', Boolean(afterSched) && schedApplied,
        `${beforeSched} -> ${afterSched || 'no new run'}; ${schedBodies.length} StartRun(s), ${schedApplied ? 'both keys sent' : (schedBodies[schedBodies.length - 1] || 'no scenario text').match(/(scheduling|buffer_max_hold_ms)[^\n]*/g)?.join(', ') || 'no scheduling/buffer_max_hold_ms'}`);
    }
    await page.close();
  }

  // b1b. GPU utilization against useful work on Issao's fleet: "when I run a simulation with 10rps
  // and 50 replicas, utilization is quite high (mean at 53%) with pretty much any policy. That
  // seems way off." Utilization is the time-in-step share and keeps that meaning; the new useful
  // tile is work over the maximum possible, and the two must disagree by the width of the batch:
  // after 30 simulated seconds useful sits in the low single digits while utilization stays over
  // half.
  {
    const { page, until } = await fresh('#/dashboard?replicas=50&arrival_rps=10&duration_s=90&warmup_s=5');
    const cursor = () => page.$eval('.playback [aria-valuenow]', el => Number(el.getAttribute('aria-valuenow'))).catch(() => NaN);
    const tile = async sel => {
      const t = await page.$eval(`[data-tile="${sel}"] .tile-value`, el => el.textContent).catch(() => '');
      const m = /(-?\d+(?:\.\d+)?)\s*%/.exec(t || '');
      return m ? Number(m[1]) : NaN;
    };
    const reached = await until(async () => (await cursor()) >= 30, 75000, 500);
    // Only the active observe tab is mounted, so the tiles exist only once Utilization is open.
    const tab = await until(() => page.$('button[data-tab="observe:utilization"]'), 4000);
    if (tab) await tab.click();
    await until(async () => Number.isFinite(await tile('gpu-utilization')), 6000, 250);
    const util = await tile('gpu-utilization');
    const useful = await tile('gpu-useful');
    const detail = `at ${await cursor()} s: utilization ${util}%, useful ${useful}%`;
    check('gpu: 50 replicas at 10 rps reach 30 simulated seconds', Boolean(reached), detail);
    check('gpu: utilization (time in step) stays above 30% at 10 rps on 50 replicas, the number Issao saw', util > 30, detail);
    check('gpu: useful work of the maximum possible is under 10% on the same fleet (Issao)', useful < 10, detail);
    await page.close();
  }

  // b2. load-test dashboard: a run that reaches STATE_COMPLETE offers Restart (U120, Issao: "add a
  // restart button when a loadtest run finishes"). The page's own default is 600 s; QA_SHORT_RUN
  // (default 20 s) overrides duration_s (and warmup_s, which sim-leaf requires stays below it) via
  // the `#/dashboard?duration_s=&warmup_s=` link config.ts's loadTestInitial reads, so the run
  // actually finishes inside this check's budget instead of the harness's.
  {
    const shortS = Number(process.env.QA_SHORT_RUN) > 0 ? Number(process.env.QA_SHORT_RUN) : 20;
    const warmupS = shortS > 1 ? 1 : 0;
    const { page, log, body, until } = await fresh(`#/dashboard?duration_s=${shortS}&warmup_s=${warmupS}`);
    const runIdOf = t => (/run (r-\d+)/.exec(t) || [])[1];
    const restartBtn = () => page.$('.playback [aria-label="restart"]');
    const startedTexts = () => log.startRuns.map(b => { try { return JSON.parse(b)?.scenario?.text ?? b; } catch { return b; } });

    await until(async () => runIdOf(await body()), 12000);
    const before = runIdOf(await body());
    check('load test: short run starts (U120 setup)', Boolean(before), before ? `run ${before}` : (await body()).slice(0, 160));

    const early = await restartBtn();
    check('load test: no Restart button while the run is in progress (U120)', early === null, early ? 'button present before completion' : 'absent, as expected');

    const complete = await until(async () => (/run complete/i.test(await body()) ? await body() : null), (shortS + 15) * 1000);
    const wantClock = `run complete · ${shortS} s simulated`;
    check('load test: end-of-run state reads "run complete · Ns simulated" (U120)', Boolean(complete) && (complete || '').includes(wantClock),
      complete ? (complete.match(/run complete[^|]{0,40}/) || [''])[0] : `never reached "run complete" within ${shortS + 15} s`);

    const restartVisible = await restartBtn();
    check('load test: Restart button appears on completion (U120)', Boolean(restartVisible), restartVisible ? 'present' : 'absent');

    const initialSeed = (/^seed = (\d+)$/m.exec(startedTexts()[0] || '') || [])[1];
    const before2 = log.startRuns.length;
    await page.click('.playback [aria-label="restart"]').catch(() => null);
    const after = await until(async () => { const id = runIdOf(await body()); return id && id !== before ? id : null; }, 15000);
    check('load test: clicking Restart starts a new run id (U120)', Boolean(after) && after !== before, `${before} -> ${after || 'no new run'}`);
    const restarted = startedTexts().slice(before2);
    const sameConfig = Boolean(initialSeed) && restarted.some(t => t.includes(`seed = ${initialSeed}`) && t.includes(`duration_s = ${shortS}`));
    check('load test: restart carries the same config and seed (U120)', sameConfig,
      `seed ${initialSeed}, ${restarted.length} StartRun(s) since click: ${(restarted[restarted.length - 1] || 'none').slice(0, 80)}`);
    const buttonGone = await until(async () => (await restartBtn()) === null, 4000, 200);
    check('load test: the new run is in progress, so Restart is gone again (U120)', Boolean(buttonGone), buttonGone ? 'gone' : 'still showing on the new run');
    await page.close();
  }

  // c. showcase on a fresh load shows the cards and starts nothing
  let titles = [];
  {
    const { page, log, body, until } = await fresh('#/showcase');
    const n = await until(async () => (await cardCount(page)) >= MIN_CARDS ? cardCount(page) : 0, 10000);
    const t = await body();
    check('showcase shows cards', n >= MIN_CARDS, `${n} cards`);
    check('showcase: no run on fresh load', !/run r-\d+/.test(t), (t.match(/run r-\d+[^|]{0,60}/) || [''])[0]);
    check('showcase: no StartRun on fresh load', !log.requests.some(u => u.endsWith('/StartRun')), `${log.startRuns.length} StartRun`);
    check('showcase: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    const showcaseBadge = await badge(page);
    check('showcase: badge empty on the card list', showcaseBadge === '', showcaseBadge);
    titles = await page.$$eval('button.card', els => els
      .filter(e => !e.disabled)
      .map(e => (e.querySelector('.card-title')?.textContent || '').trim()).filter(Boolean));
    await page.close();
  }

  // U102 (Issao: "include a 'play' button there that resumes the scenario at the predetermined
  // speed, the card should show 'advancing scenario' until the next stop point shows up. Using the
  // play button in the play bar should have the same effect."). One settle, then one step from
  // each button, each judged the same way: the body says advancing, then the next title appears.
  const playFlow = async (page, until) => {
    const id = new URLSearchParams(page.url().split('?')[1] || '').get('script');
    const script = await page.evaluate(async i => (await fetch(`walkthroughs/${i}.json`)).json(), id).catch(() => null);
    if (!script) { check('showcase: play advances the scenario (U102)', false, `no script json for ${id}`); return; }
    const steps = script.steps;
    const title = () => page.$eval('.walkthrough .wt-title', el => el.textContent.trim()).catch(() => '');
    const bodyOf = () => page.$eval('.walkthrough', el => el.innerText.replace(/\s+/g, ' ')).catch(() => '');
    const cursor = () => page.$eval('.playback [aria-valuenow]', el => Number(el.getAttribute('aria-valuenow'))).catch(() => 0);
    const settled = i => until(async () => (await title()) === steps[i].title, (steps[i].at_sim_s / (steps[i].speed || 1)) * 1.5 * 1000 + 5000);
    const notTried = why => { check('showcase: play advances the scenario (U102)', false, why); check('showcase: bar play is the same action (U102)', false, why); };
    if (!(await settled(0))) return notTried(`first stop never settled: title "${await title()}"`);
    // The runner pauses the run at the stop; the bar must agree (its label comes from the server's
    // status) and the cursor must hold, or a "play" below is not what moves the run. With ~15
    // subscriptions open over HTTP/1.1 the browser's six-per-host limit starves every other RPC,
    // and a pause that never lands looks exactly like a walkthrough that plays itself.
    const barLabel = () => page.$eval('.playback .btn.primary', el => el.getAttribute('aria-label')).catch(() => '');
    const pausedOnBar = await until(async () => (await barLabel()) === 'play', 4000);
    const c0 = await cursor(); await sleep(1500); const c1 = await cursor();
    check('showcase: the run pauses at the stop point (U102)', Boolean(pausedOnBar) && c1 === c0,
      `bar says "${await barLabel()}", cursor ${c0} -> ${c1} over 1.5 s`);
    if (!pausedOnBar || c1 !== c0) return notTried('not tried: the run did not pause at the first stop (controls are not reaching the server)');
    const stepFrom = async (i, click, label) => {
      const from = await cursor();
      const next = steps[i + 1];
      await click();
      const advancing = await until(async () => /advancing scenario/.test(await bodyOf()), 1000);
      const arrived = await until(async () => (await title()) === next.title, ((next.at_sim_s - from) / (next.speed || 1)) * 1.5 * 1000 + 5000);
      check(label, Boolean(advancing && arrived), [
        advancing ? 'advancing shown' : `no "advancing scenario" within 1 s: "${(await bodyOf()).slice(0, 120)}"`,
        arrived ? `arrived at "${next.title}"` : `title "${await title()}", wanted "${next.title}"`,
      ].join(' · '));
      return advancing && arrived;
    };
    const cardPlay = () => page.click('.walkthrough .wt-play');
    const barPlay = () => page.click('.playback [aria-label="play"]');
    if (await stepFrom(0, cardPlay, 'showcase: play advances the scenario (U102)')) {
      if (await until(async () => (await barLabel()) === 'play', 4000)) await stepFrom(1, barPlay, 'showcase: bar play is the same action (U102)');
      else check('showcase: bar play is the same action (U102)', false, `not tried: the bar never showed play after the second stop (label "${await barLabel()}")`);
    } else {
      check('showcase: bar play is the same action (U102)', false, 'not tried: the card play flow failed');
    }
  };

  // U103 (Issao: "can you make the showcase card draggable?"). Dragged by the header (not a
  // button in it), pointer events so touch works too; the offset must outlive the next step (the
  // runner swaps the card's content, not the element) and reset must put it back exactly.
  const dragFlow = async (page, until) => {
    const box = () => page.$eval('.walkthrough', el => { const r = el.getBoundingClientRect(); return { x: r.x, y: r.y }; });
    const titleText = () => page.$eval('.wt-title', el => el.textContent.trim()).catch(() => '');
    const headPoint = () => page.$eval('.walkthrough .wt-title', el => {
      const r = el.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
    });
    const b0 = await box();
    const p0 = await headPoint();
    // The card sits flush against the bottom-right corner (`.walkthrough`'s natural position), so
    // a drag has to head up-left, into the open viewport, or the clamp -- correctly -- eats most
    // of it.
    await page.mouse.move(p0.x, p0.y);
    await page.mouse.down();
    for (const [dx, dy] of [[-60, -35], [-130, -75], [-200, -120]]) await page.mouse.move(p0.x + dx, p0.y + dy, { steps: 5 });
    await page.mouse.up();
    const b1 = await box();
    check('showcase: card drags by its header (U103)', Math.abs(b1.x - b0.x + 200) <= 2 && Math.abs(b1.y - b0.y + 120) <= 2,
      `moved by (${(b1.x - b0.x).toFixed(1)}, ${(b1.y - b0.y).toFixed(1)}), wanted (-200, -120)`);
    const beforeTitle = await titleText();
    await page.click('.walkthrough .wt-play').catch(() => null);
    await until(async () => (await titleText()) !== beforeTitle, 4000);
    const b2 = await box();
    check('showcase: card stays where dropped across steps (U103)', Math.abs(b2.x - b1.x) <= 1 && Math.abs(b2.y - b1.y) <= 1,
      `(${b2.x.toFixed(1)}, ${b2.y.toFixed(1)}) vs dropped at (${b1.x.toFixed(1)}, ${b1.y.toFixed(1)})`);
    await page.click('.walkthrough .wt-reset');
    // Not a pixel match against b0: the card's own content (and so its bottom-anchored natural
    // height) is longer now than at the first step, so the natural position has legitimately moved
    // since b0 was taken. Reset's contract is "no more override", not "back to that old pixel" --
    // checked directly as the absence of the inline left/top useDraggable pins with.
    const pin = await page.$eval('.walkthrough', el => el.style.left || null);
    check('showcase: reset returns the card to its natural position (U103)', pin === null,
      pin === null ? 'no left/top override left behind' : `still pinned at left=${pin}`);
  };

  // d. every scripted card drives a live run
  let open = null; // a page with a walkthrough open, for the navigation check
  for (const title of titles) {
    const { page, log, body, until } = await fresh('#/showcase');
    await until(() => cardCount(page), 10000);
    const card = await page.$(`button.card:has(.card-title:text-is("${title.replace(/"/g, '\\"')}"))`);
    if (!card) { check(`showcase "${title}"`, false, 'card not found'); await page.close(); continue; }
    await card.click();
    // A run id and a first sample, as soon as they show; a card is stuck only when 20 s pass
    // without a sample, so a slow cold start on Cloud Run is not a failure and a fast one is not
    // a 10 s wait.
    await until(async () => {
      const b = await body();
      const m = /(\d+) samples/.exec(b);
      return /run r-\d+/.test(b) && m !== null && Number(m[1]) > 0;
    }, 20000);
    const t = await body();
    const mode = await page.$eval('.wt-mode', el => el.textContent.trim()).catch(() => '');
    const cardBadge = await badge(page);
    const stuck = /waiting for the first sample|looking for/i.test(t);
    const badgeOk = /^live — .*run r-\d+/.test(cardBadge || '');
    const ok = !stuck && /driving a live run/.test(mode) && /run r-\d+/.test(t) && badgeOk && log.errs.length === 0 && log.bad.length === 0;
    check(`showcase "${title}"`, ok, [
      stuck ? 'STUCK' : '', `mode="${mode.slice(0, 40)}"`, `badge="${(cardBadge || '').slice(0, 50)}"`,
      (t.match(/run r-\d+[^|]{0,50}/) || ['no run'])[0],
      ...log.errs.slice(0, 2), ...log.bad.slice(0, 2),
    ].filter(Boolean).join(' · '));
    // The first card only, to keep the run short: every card drives the same dashboard.
    // U102 first: the card's Play resumes the run at the step's speed and the card reads
    // "advancing scenario…" until the next stop; the playback bar's play is the same action. The
    // tab walk comes after, because the Machine tab's replica subscriptions can fill the browser's
    // per-host connection limit and a control issued behind them waits indefinitely.
    const drove = ok && title === titles[0];
    if (drove) await playFlow(page, until);
    if (drove) await dragFlow(page, until);
    if (drove) await noInvented(page, 'showcase: no invented numbers on a live run (U95b)');
    if (REQUIRED_KEYS[title]) {
      const bodies = log.startRuns.map(b => { try { return JSON.parse(b)?.scenario?.text ?? b; } catch { return b; } });
      for (const re of REQUIRED_KEYS[title]) {
        check(`showcase "${title}" sends ${re.source.split('\\s')[0]}`, bodies.some(b => re.test(b)),
          bodies.length ? bodies[bodies.length - 1].slice(0, 160) : 'no StartRun captured');
      }
    }
    // Follow-up to 8a425fe/the prefill_wait fix: a *preempted* traced request left a gap too, since
    // nothing emitted a `preempted` span and re-admission never chained onto the next one. This
    // card is where a preemption spiral is the whole point, so it is where the harness reads the
    // engine's own spans and demands the same no-gap invariant `prefill_wait` already has. Its
    // scenario is `preemption = never` (checked above, and finding 8: 0 preemptions/s for `never`),
    // so a preempted span existing here would itself be the invented number; the count is reported,
    // not required, the same way the dashboard's own no-gap check reports `prefill_wait` spans
    // without requiring at least one.
    if (title === 'KV preemption spiral at low load') {
      const runId = ((cardBadge || '').match(/run (r-\d+)/) || [])[1];
      const finished = new Set(['OUTCOME_OK', 'OUTCOME_OK_SLO_VIOLATED']);
      let traces = [];
      if (runId) {
        const r = await ctx.request.post(BASE + '/v1/ingress/GetTraces', { data: { run_id: runId, limit: 20 } }).catch(() => null);
        traces = r && r.ok() ? ((await r.json().catch(() => ({}))).traces || []) : [];
      }
      const gaps = [];
      let preempted = 0;
      for (const t of traces) {
        if (!finished.has(t.record && t.record.outcome)) continue;
        const spans = (t.spans || []).map(s => ({ op: s.operation, start: BigInt(s.start_unix_ns), end: BigInt(s.end_unix_ns) }));
        for (let i = 1; i < spans.length; i++) {
          const gapMs = Number(spans[i].start - spans[i - 1].end) / 1e6;
          if (gapMs > 100) gaps.push(`${t.record.id}: ${gapMs.toFixed(0)} ms between ${spans[i - 1].op} and ${spans[i].op}`);
          if (spans[i].op === 'preempted') preempted++;
        }
      }
      check('showcase "KV preemption spiral at low load": no gap over 100 ms between consecutive spans',
        Boolean(runId) && traces.length > 0 && gaps.length === 0,
        !runId ? 'no run id on the card badge' : traces.length === 0 ? 'GetTraces returned no traces' : gaps.length ? gaps.slice(0, 3).join(' · ') : `${traces.length} traces, ${preempted} preempted spans, contiguous`);
    }
    // A driven page may still hold a control in flight when its run is stopped below; that lands
    // as a 409 in a console the nav check reads, so the nav page is the next clean card.
    if (ok && !open && !drove) open = { page, log, until }; else await page.close();
    await stopRuns();
  }

  // e. the nav link leaves the walkthrough; the browser's back button returns to it
  {
    if (!open) {
      const { page, log, until } = await fresh('#/showcase');
      await until(() => cardCount(page), 10000);
      const card = await page.$('button.card');
      if (card) { await card.click(); await sleep(5000); }
      open = { page, log, until };
    }
    const { page, log, until } = open;
    const link = await page.$('a[href="#/showcase"]');
    if (!link) check('nav: Showcase link present', false, 'no a[href="#/showcase"]');
    else {
      await link.click();
      const back = await until(async () => (await cardCount(page)) >= MIN_CARDS && !(await hasWt(page)), 2000);
      check('nav: Showcase link returns to cards', back, `${await cardCount(page)} cards, wt-mode ${await hasWt(page) ? 'present' : 'gone'}`);
      await page.goBack();
      const again = await until(() => hasWt(page), 3000);
      check('nav: back restores the walkthrough (U75)', again, `url ${page.url()}`);
    }
    check('nav: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }

  // f. A/B live, then explicit replay
  {
    // U96: on a server the A/B view is two live runs from one scenario text and one seed, differing
    // only in policy; the harness reads the ids the page shows and the seeds the page actually sent.
    const { page, log, body, until } = await fresh('#/ab');
    const runPair = async () => { const m = (await body()).match(/run ([A-Za-z0-9_-]+) vs ([A-Za-z0-9_-]+)/); return m ? [m[1], m[2]] : null; };
    const ids = await until(runPair, 20000);
    const t = await body();
    // Case-insensitive: the tagline is upper-cased by CSS, and innerText reports the rendered case.
    const seedShown = (t.match(/same seed (\d+)/i) || [])[1];
    const sent = log.startRuns.map(b => (b.match(/seed\s*=\s*(\d+)/) || [])[1]);
    const routings = log.startRuns.map(b => (b.match(/routing\s*=\s*([a-z_0-9]+)/) || [])[1]);
    check('A/B live: two runs streaming with equal seeds',
      Boolean(ids) && ids[0] !== ids[1] && sent.length === 2 && sent[0] && sent[0] === sent[1] && sent[0] === seedShown && routings[0] !== routings[1],
      `ids ${JSON.stringify(ids)}, seeds sent ${JSON.stringify(sent)}, shown ${seedShown}, routing ${JSON.stringify(routings)}`);
    const cursor = () => page.$eval('[role=slider]', el => Number(el.getAttribute('aria-valuenow'))).catch(() => NaN);
    const c0 = await until(async () => { const c = await cursor(); return c > 0 ? c : null; }, 15000);
    await sleep(4000);
    const c1 = await cursor();
    check('A/B live: cursor advances', c1 - c0 >= 2, `${c0} -> ${c1} s over 4 s`);
    const nonZero = async () => {
      const cells = await page.$$eval('.diff-table td.n', els => els.map(e => e.textContent.trim())).catch(() => []);
      return cells.some(c => /[1-9]/.test(c)) ? cells : null;
    };
    // The difference is read once both runs have ten seconds of samples; before that a zero row is honest.
    await until(async () => (await cursor()) >= 10, 30000);
    const cells = await until(nonZero, 10000);
    check('A/B live: difference panel non-zero', Boolean(cells), cells ? cells.slice(0, 6).join(' ') : (await body()).slice(0, 160));
    const abBadge = await badge(page);
    check('A/B live: badge live', /^live — /.test(abBadge || ''), abBadge);
    check('A/B live: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }
  {
    const { page, log, body, until } = await fresh('?server=off#/dashboard');
    const t = await until(async () => { const b = await body(); return /replay/i.test(b) && !/waiting for the first sample/i.test(b) ? b : null; }, 8000) || await body();
    check('replay dashboard (server=off)', /replay/i.test(t) && !/waiting for the first sample/i.test(t), t.slice(0, 140));
    const replayBadge = await badge(page);
    check('replay: badge replay', /^replay — /.test(replayBadge || ''), replayBadge);
    await machines(page, until, 'replay');
    await noInvented(page, 'replay: no invented numbers on a recording (U100)');
    const loadTabBtn = await page.$('#control button:has-text("Load")');
    if (loadTabBtn) await loadTabBtn.click();
    const loadDisabled = await until(async () => {
      const d = await page.$eval('#control fieldset[disabled]', el => el.textContent).catch(() => null);
      return d && /recording/i.test(d) ? d : null;
    }, 3000);
    check('replay: load tab disabled as recording', Boolean(loadDisabled && /recording/i.test(loadDisabled)),
      loadDisabled ? loadDisabled.slice(0, 120) : 'no fieldset[disabled] in #control');
    // Smoothing on a recording is the same selector, applied client-side to the recorded frames; the
    // readout beside the latency chart says which window its percentiles are over.
    {
      await page.click('.playback [data-smooth="30s"]');
      await page.click('button[data-tab="observe:quality"]').catch(() => null);
      const readout = await until(async () => { const t = await page.$eval('#ttft .panel-sub', el => el.textContent).catch(() => ''); return /30 s window/.test(t) ? t : null; }, 5000);
      check('smoothing: replay readout says 30 s window (Issao)', Boolean(readout), readout || (await page.$eval('#ttft .panel-sub', el => el.textContent).catch(() => 'no #ttft panel')));
      const pts = await page.$eval('#ttft path[data-key="p99"]', el => el.getAttribute('data-values').split(',').filter(Boolean).length).catch(() => 0);
      check('smoothing: replay p99 series still drawn point for point', pts >= 2, `${pts} points`);
      await page.click('.playback [data-smooth="off"]');
    }
    check('replay: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }
  // U100: with neither a server nor a recording the page says so in words and draws nothing; the
  // in-browser stand-in that used to fill this gap with invented replicas is gone.
  {
    const { page, log, body, until } = await fresh('?server=off&replay=off#/dashboard');
    const t = await until(async () => { const b = await body(); return /no server and no recordings served/.test(b) ? b : null; }, 8000) || await body();
    check('no source: dashboard says so (U100)', /no server and no recordings served/.test(t) && !/mock/i.test(t), t.slice(0, 140));
    const noneBadge = await badge(page);
    check('no source: badge empty', noneBadge === '', noneBadge);
    const tiles = await page.$$eval('.tile, .panel', els => els.length);
    check('no source: nothing drawn', tiles === 0, `${tiles} tiles/panels`);
    check('no source: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }

  // g. utilization: the GPU panel with its fleet mean (U94). The tile reads '-' until the wire
  // carries metric 67 (U94b), so these two fail honestly on a build that predates it.
  for (const [label, hash] of [['live', '#/dashboard'], ['replay', '?server=off#/dashboard']]) {
    const { page, until } = await fresh(hash);
    await until(() => page.$$eval('button', bs => bs.some(b => /^Utilization$/.test(b.textContent.trim()))), 12000);
    await page.$$eval('button', bs => { const b = bs.find(b => /^Utilization$/.test(b.textContent.trim())); if (b) b.click(); });
    const gpu = await until(() => page.$('#gpu'), 8000);
    check(`utilization ${label}: gpu panel exists (U94)`, Boolean(gpu), gpu ? '#gpu' : 'no #gpu');
    // The tile's value is a percentage; a dash means the frame carried NaN, a gap rather than a number.
    const tile = async () => page.$$eval('#capacity *', els => {
      const lab = els.find(e => e.children.length === 0 && /^gpu utilization$/i.test((e.textContent || '').trim()));
      const box = lab && lab.parentElement;
      return box ? box.innerText.replace(/\s+/g, ' ') : '';
    }).catch(() => '');
    const txt = await until(async () => { const t = await tile(); return /\d+(\.\d+)?%/.test(t) ? t : null; }, 8000) || await tile();
    const pct = Number((txt.match(/(\d+(?:\.\d+)?)%/) || [0, '0'])[1]);
    check(`utilization ${label}: gpu chart with non-zero mean (U94)`, pct > 0, txt.slice(0, 80) || 'no gpu tile');
    if (label === 'live') {
      // U115 (Issao: a full cache showed no preemptions). The KV panel states the mode the run is
      // under, and the tile carries a number once the wire serves metric 46: zero is allowed, a
      // fleet with headroom evicts nothing, so the check is presence, not magnitude.
      const kvText = await page.$eval('#kv', e => e.innerText.replace(/\s+/g, ' ')).catch(() => '');
      check('utilization: kv panel states the preemption mode (U115)', /preemption: swap/i.test(kvText), kvText.slice(0, 120));
      const preTile = async () => page.$$eval('#capacity *', els => {
        const lab = els.find(e => e.children.length === 0 && /^preemptions$/i.test((e.textContent || '').trim()));
        const box = lab && lab.parentElement;
        return box ? box.innerText.replace(/\s+/g, ' ') : '';
      }).catch(() => '');
      await sleep(10000);
      const pre = await until(async () => { const t = await preTile(); return /\d+(\.\d+)? \/s/.test(t) ? t : null; }, 10000) || await preTile();
      const rate = Number((pre.match(/(\d+(?:\.\d+)?) \/s/) || [0, 'NaN'])[1]);
      check('utilization: preemptions per second is a number, not blank (U115)', Number.isFinite(rate), pre.slice(0, 80) || 'no preemptions tile');
    }
    await page.close();
  }

  // h1. service quality: the badput chart. Issao, 2026-09-08: "for service quality, include a
  // goodput graph that is a function of throughput, and use a log scale ... (i.e. show
  // (1-goodput/throughput) i.e. badput percentage in log scale)". The panel is #badput; its
  // readout is the tile data-tile="badput".
  for (const [label, hash] of [['live', '#/dashboard'], ['replay', '?server=off#/dashboard']]) {
    const { page, until } = await fresh(hash);
    await until(() => page.$$eval('button', bs => bs.some(b => /^Service quality$/.test(b.textContent.trim()))), 12000);
    await page.click('button[data-tab="observe:quality"]').catch(() => null);
    const panel = await until(() => page.$('#badput'), 10000);
    check(`service quality ${label}: badput panel exists`, Boolean(panel), panel ? '#badput' : 'no #badput');
    const hasPoint = await until(
      () => page.$eval('#badput svg path[d]', el => (el.getAttribute('d') || '').length > 5).catch(() => false),
      15000
    );
    check(`service quality ${label}: badput chart has at least one point`, Boolean(hasPoint), String(hasPoint));
    // Both sources autoplay from a fresh page, and the cursor starts at (or near) 0: a window with
    // no completions yet has no throughput to divide by, so `fmtSig2Pct` prints "-" rather than a
    // false zero (derive.ts's `badput`, ServiceQuality.tsx). That is correct, not missing, so this
    // waits for the cursor to carry the run past its first completions instead of reading the tile
    // at whatever instant the page happened to be on.
    const readTile = () => page.$eval('[data-tile="badput"]', el => el.innerText.replace(/\s+/g, ' ')).catch(() => '');
    const readout = await until(async () => { const t = await readTile(); return /\d+(\.\d+)?%/.test(t) ? t : null; }, 20000) || await readTile();
    check(`service quality ${label}: badput readout ends in a percentage`, /\d+(\.\d+)?%/.test(readout), readout.slice(0, 120) || 'no [data-tile="badput"]');
    await page.close();
  }

  // h. layout stability (U99): a card's height must not change because text -- the wasted-GPU
  // note, a refusal, a connection phase, a sample count -- happened to show up or disappear this
  // frame. Boxes are measured three times, five seconds apart, and compared as-is.
  const layoutBoxes = page => page.$$eval('.tile, .panel, .banner, .statusbar', els => els.map(e => {
    const r = e.getBoundingClientRect();
    return [e.className, e.id, Math.round(r.height), Math.round(r.width)];
  }));
  const layoutDiffs = samples => samples[0]
    .map((b, i) => [b, ...samples.slice(1).map(s => s[i])])
    .filter(row => row.some(box => !box || box[2] !== row[0][2] || box[3] !== row[0][3]));
  {
    const { page, body, until } = await fresh('#/dashboard');
    await until(async () => { const b = await body(); return !/waiting for the first sample/i.test(b) ? b : null; }, 12000);
    await page.click('button[data-tab="observe:quality"]').catch(() => null);
    await sleep(300);
    const boxSamples = [];
    const wastedHeights = [];
    for (let i = 0; i < 3; i++) {
      boxSamples.push(await layoutBoxes(page));
      wastedHeights.push(await page.$eval('[data-tile="wasted"]', el => Math.round(el.getBoundingClientRect().height)).catch(() => null));
      if (i < 2) await sleep(5000);
    }
    const diffs = layoutDiffs(boxSamples);
    check('layout: no card changes height on the live dashboard (U99)', diffs.length === 0, JSON.stringify(diffs.slice(0, 5)));
    check('layout: service quality headline wasted tile keeps its height (U99)',
      wastedHeights.every(h => h !== null && h === wastedHeights[0]), JSON.stringify(wastedHeights));
    await page.close();
  }
  {
    const { page, until } = await fresh('#/showcase');
    await until(() => cardCount(page), 10000);
    const card = await page.$('button.card');
    if (!card) {
      check('layout: no card changes height on the showcase first card (U99)', false, 'no card found');
    } else {
      await card.click();
      await until(async () => (await layoutBoxes(page)).length > 0, 15000);
      const boxSamples = [];
      for (let i = 0; i < 3; i++) {
        boxSamples.push(await layoutBoxes(page));
        if (i < 2) await sleep(5000);
      }
      const diffs = layoutDiffs(boxSamples);
      check('layout: no card changes height on the showcase first card (U99)', diffs.length === 0, JSON.stringify(diffs.slice(0, 5)));
    }
    await page.close();
    await stopRuns();
  }

  // i. app shell (U107). Issao: "the scrolling of control seems wrong, i think it should scroll
  // inside the panel, not the whole page, otherwise the 'subscriptions open...' line at the bottom
  // stays over it." The document never scrolls on the dashboard; the control panel's body is its
  // own scroll container; the status bar is a row below it, not a bar over it.
  {
    const { page, body, until } = await fresh('#/dashboard');
    // At 1400x900 the Cluster tab fits and the checks would pass without scrolling anything.
    await page.setViewportSize({ width: 1280, height: 720 });
    // The shell exists only once the first sample has arrived; on the cloud instance that can take
    // longer than the 12 s the text wait allowed, and the checks then ran against the placeholder.
    // Wait for the panel body itself, up to 40 s.
    await page.waitForSelector('#control .panel-body', { timeout: 40000 }).catch(() => undefined);
    await until(async () => !/waiting for the first sample/i.test(await body()), 5000);
    const tab = await page.$('button[data-tab="control:cluster"]');
    if (tab) await tab.click();
    await sleep(400);
    const doc = await page.evaluate(() => ({ sh: document.scrollingElement.scrollHeight, ih: window.innerHeight }));
    check('shell: page does not scroll (U107)', doc.sh <= doc.ih + 1, `document ${doc.sh}px tall in a ${doc.ih}px window`);
    const box = await page.$eval('#control .panel-body', el => {
      const r = el.getBoundingClientRect();
      return { x: r.x + r.width / 2, y: r.y + r.height / 2, sh: el.scrollHeight, ch: el.clientHeight };
    }).catch(() => null);
    if (box) { await page.mouse.move(box.x, box.y); await page.mouse.wheel(0, 400); await sleep(400); }
    const after = box ? await page.evaluate(() => ({ top: document.querySelector('#control .panel-body').scrollTop, y: window.scrollY })) : null;
    check('shell: control panel scrolls inside itself (U107)', Boolean(box && box.sh > box.ch && after.top > 0 && after.y === 0),
      box ? `body ${box.sh}px in ${box.ch}px; after a 400px wheel: panel scrollTop ${after.top}, window scrollY ${after.y}` : 'no #control .panel-body');
    const gap = await page.evaluate(() => {
      const b = document.querySelector('#control .panel-body');
      const bar = document.querySelector('.statusbar');
      if (!b || !bar) return null;
      b.scrollTop = b.scrollHeight;
      const controls = b.querySelectorAll('input, select, button');
      const last = controls[controls.length - 1];
      return last ? { last: last.getBoundingClientRect().bottom, bar: bar.getBoundingClientRect().top } : null;
    });
    check('shell: status bar does not overlap the last control (U107)', Boolean(gap && gap.last <= gap.bar + 0.5),
      gap ? `last control ends at ${gap.last.toFixed(0)}px, status bar starts at ${gap.bar.toFixed(0)}px` : 'no last control or no status bar');
    await page.close();
  }

  await stopRuns();
  // Best effort: the server's own view of what the harness left running.
  try {
    const listed = await (await ctx.request.post(BASE + '/v1/ingress/ListRuns', { data: {} })).json();
    const running = (listed.runs || []).filter(r => started.has(r.run_id) && String(r.state) === 'STATE_RUNNING');
    check('harness stopped its runs', running.length === 0, `${started.size} started, ${running.length} still running`);
  } catch (e) {
    check('harness stopped its runs', false, `ListRuns: ${String(e && e.message || e).slice(0, 120)}`);
  }
  await browser.close();
  console.log(finalLine(0));
  process.exit(results.every(Boolean) ? 0 : 1);
})().catch(e => { console.log(`FAIL harness — ${String(e && e.message || e).slice(0, 300)}`); console.log(finalLine(1)); process.exit(1); });
