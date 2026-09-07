// Browser gate for the dashboard. Drives a real Chromium against a running server (local
// `sim-run serve` or lbsim.ai) and asserts the target state of the product, not today's: a check
// that fails here is a unit of work someone owes, which is why every check names what it saw.
//
// Usage: QA_BASE=http://localhost:8181 node tools/qa/qa.js   (tools/qa/serve-local.sh wraps this)
const { chromium } = require('playwright-core');
const fs = require('fs');

const BASE = process.env.QA_BASE || 'http://localhost:8181';
const MIN_CARDS = 14;
const EXPECTED_REPORTS = 12;
// Two cards whose scenario overrides are the whole point of the demo; a walkthrough that runs
// without them looks fine and shows nothing.
const REQUIRED_KEYS = {
  'KV preemption spiral at low load': [/preemption\s*=\s*never\b/, /session_turns_mean\s*=\s*8\b/],
  'Speculative decoding value and cost': [/spec_draft_tokens\s*=\s*4\b/],
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
    const log = { errs: [], bad: [], startRuns: [], requests: [] };
    page.on('pageerror', e => log.errs.push(e.message.slice(0, 200)));
    page.on('console', m => { if (m.type() === 'error' && !m.text().includes('SetSpeed')) log.errs.push('console: ' + m.text().slice(0, 200)); });
    page.on('response', r => {
      if (r.status() >= 400 && !r.url().includes('SetSpeed')) log.bad.push(`${r.status()} ${r.url().slice(0, 100)}`);
      if (r.url().endsWith('/StartRun') && r.status() === 200) r.json().then(j => { if (j && j.run_id) started.add(j.run_id); }).catch(() => null);
    });
    page.on('request', r => {
      log.requests.push(r.url());
      if (r.url().endsWith('/StartRun') && r.postData()) log.startRuns.push(r.postData());
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
    check('home: twelve reports', hrefs.length === EXPECTED_REPORTS, `${hrefs.length} links`);
    check('home: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }

  // b. live dashboard
  {
    const { page, log, body, until } = await fresh('#/dashboard');
    const ready = t => !/waiting for the first sample/i.test(t) && /\blive\b/i.test(t) && /run r-\d+/.test(t);
    let t = await until(async () => { const b = await body(); return ready(b) ? b : null; }, 12000) || await body();
    check('dashboard renders samples', !/waiting for the first sample/i.test(t), t.slice(0, 160));
    check('dashboard says live', /\blive\b/i.test(t) && /run r-\d+/.test(t), (t.match(/run r-\d+[^|]{0,60}/) || [''])[0]);
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
    titles = await page.$$eval('button.card', els => els
      .filter(e => !/not scripted yet/.test(e.textContent || ''))
      .map(e => (e.querySelector('.card-title')?.textContent || '').trim()).filter(Boolean));
    await page.close();
  }

  // d. every scripted card drives a live run
  let open = null; // a page with a walkthrough open, for the navigation check
  for (const title of titles) {
    const { page, log, body, until } = await fresh('#/showcase');
    await until(() => cardCount(page), 10000);
    const card = await page.$(`button.card:has(.card-title:text-is("${title.replace(/"/g, '\\"')}"))`);
    if (!card) { check(`showcase "${title}"`, false, 'card not found'); await page.close(); continue; }
    await card.click();
    await sleep(10000);
    const t = await body();
    const mode = await page.$eval('.wt-mode', el => el.textContent.trim()).catch(() => '');
    const stuck = /waiting for the first sample|looking for/i.test(t);
    const ok = !stuck && /driving a live run/.test(mode) && /run r-\d+/.test(t) && log.errs.length === 0 && log.bad.length === 0;
    check(`showcase "${title}"`, ok, [
      stuck ? 'STUCK' : '', `mode="${mode.slice(0, 40)}"`, (t.match(/run r-\d+[^|]{0,50}/) || ['no run'])[0],
      ...log.errs.slice(0, 2), ...log.bad.slice(0, 2),
    ].filter(Boolean).join(' · '));
    if (REQUIRED_KEYS[title]) {
      const bodies = log.startRuns.map(b => { try { return JSON.parse(b)?.scenario?.text ?? b; } catch { return b; } });
      for (const re of REQUIRED_KEYS[title]) {
        check(`showcase "${title}" sends ${re.source.split('\\s')[0]}`, bodies.some(b => re.test(b)),
          bodies.length ? bodies[bodies.length - 1].slice(0, 160) : 'no StartRun captured');
      }
    }
    if (ok && !open) open = { page, log, until }; else await page.close();
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

  // f. A/B and explicit replay
  {
    const { page, log, body, until } = await fresh('#/ab');
    const t = await until(async () => { const b = await body(); return b.length > 200 && !/waiting for the first sample/i.test(b) ? b : null; }, 8000) || await body();
    check('A/B renders', t.length > 200 && !/waiting for the first sample/i.test(t), t.slice(0, 120));
    check('A/B: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
    await page.close();
  }
  {
    const { page, log, body, until } = await fresh('?server=off#/dashboard');
    const t = await until(async () => { const b = await body(); return /replay/i.test(b) && !/waiting for the first sample/i.test(b) ? b : null; }, 8000) || await body();
    check('replay dashboard (server=off)', /replay/i.test(t) && !/waiting for the first sample/i.test(t), t.slice(0, 140));
    check('replay: no js errors', log.errs.length === 0, log.errs.slice(0, 3).join(' | '));
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
