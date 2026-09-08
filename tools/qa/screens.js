// Screenshot walkthrough of the dashboard. Drives a real Chromium against a running server and
// saves one full-page PNG per route (and per control tab on the dashboard), so a UI change can be
// reviewed side by side against the previous set instead of described. It asserts nothing.
//
// Usage: QA_BASE=http://localhost:8181 QA_SHOTS=/tmp/lbsim-screens node tools/qa/screens.js
//   (QA_SCRIPT=screens.js tools/qa/serve-local.sh wraps this)
const { chromium } = require('playwright-core');
const fs = require('fs');
const path = require('path');

const BASE = process.env.QA_BASE || 'http://localhost:8181';
const OUT = process.env.QA_SHOTS || '/tmp/lbsim-screens';
// U99: instead of the route walkthrough below, measure card/panel/banner/statusbar boxes three
// times five seconds apart and report which ones changed size -- the same jump a person would see.
const STABILITY = process.env.QA_STABILITY === '1';
const CHROME = [
  '/home/agents/.cache/ms-playwright/chromium-1140/chrome-linux/chrome',
  '/home/agents/.cache/ms-playwright/chromium_headless_shell-1234/chrome-linux/headless_shell',
  '/home/agents/.cache/ms-playwright/chromium-1234/chrome-linux/chrome',
].find(p => fs.existsSync(p));
const CONTROL_TABS = ['scenarios', 'load', 'policies', 'cluster', 'run'];

const sleep = ms => new Promise(r => setTimeout(r, ms));
setTimeout(() => { console.log('screens: global timeout'); process.exit(1); }, 5 * 60 * 1000).unref();

(async () => {
  if (!CHROME) throw new Error('no Chromium under ~/.cache/ms-playwright');
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch({ executablePath: CHROME, args: ['--no-sandbox'] });
  const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } });

  // Every run this walkthrough starts is its own to stop; the server caps live runs.
  const started = new Set();
  ctx.on('response', r => {
    if (r.url().endsWith('/StartRun') && r.status() === 200) r.json().then(j => { if (j && j.run_id) started.add(j.run_id); }).catch(() => null);
  });
  const stopRuns = async () => {
    for (const id of started) await ctx.request.post(BASE + '/v1/ingress/StopRun', { data: { run_id: id } }).catch(() => null);
    started.clear();
  };

  const shot = async (page, name) => {
    const file = path.join(OUT, name + '.png');
    await page.screenshot({ path: file, fullPage: true });
    console.log(file);
  };
  const open = async (hash, settleMs) => {
    const page = await ctx.newPage();
    await page.goto(BASE + '/' + hash, { waitUntil: 'domcontentloaded', timeout: 30000 });
    await sleep(settleMs);
    return page;
  };
  // A live run needs its first samples before the page is worth looking at; poll rather than
  // sleep so a healthy page moves on quickly.
  const untilSamples = async (page, ms) => {
    const end = Date.now() + ms;
    for (;;) {
      const t = await page.evaluate(() => document.body.innerText).catch(() => '');
      if (!/waiting for the first sample/i.test(t) && t.length > 200) return;
      if (Date.now() > end) return;
      await sleep(250);
    }
  };
  // U99: the boxes a jumpy layout would resize. Compared as [className, id, height, width] so a
  // diff names exactly which element and which dimension moved.
  const measureBoxes = page => page.evaluate(() => [...document.querySelectorAll('.tile, .panel, .banner, .statusbar')].map(e => {
    const r = e.getBoundingClientRect();
    return [e.className, e.id, Math.round(r.height), Math.round(r.width)];
  }));
  const diffBoxes = samples => samples[0]
    .map((b, i) => [b, ...samples.slice(1).map(s => s[i])])
    .filter(row => row.some(box => !box || box[2] !== row[0][2] || box[3] !== row[0][3]));
  const stability = async (page, name) => {
    const samples = [];
    for (let i = 0; i < 3; i++) {
      samples.push(await measureBoxes(page));
      await shot(page, `${name}-stability-${i}`);
      if (i < 2) await sleep(5000);
    }
    const diffs = diffBoxes(samples);
    console.log(`stability ${name}: ${diffs.length} box(es) differ across the three samples`);
    if (diffs.length) console.log(JSON.stringify(diffs, null, 2));
  };

  try {
    if (STABILITY) {
      {
        const page = await open('#/dashboard', 500);
        await untilSamples(page, 15000);
        await stability(page, 'dashboard');
        await page.close();
      }
      {
        const page = await open('#/showcase', 1500);
        const card = await page.$('button.card');
        if (card) {
          await card.click();
          await untilSamples(page, 20000);
          await stability(page, 'showcase-card');
        } else {
          console.log('stability showcase-card: no card found');
        }
        await page.close();
      }
      return; // the outer `finally` still stops runs and closes the browser
    }
    {
      const page = await open('#/', 1500);
      await shot(page, 'home');
      await page.close();
    }
    {
      const page = await open('#/dashboard', 500);
      await untilSamples(page, 15000);
      await sleep(3000);
      // Two sizes: the shell layout has to hold where the control panel overflows (720 tall), not
      // only where it fits.
      await shot(page, 'dashboard-1400x900');
      await page.setViewportSize({ width: 1280, height: 720 });
      await sleep(400);
      await shot(page, 'dashboard-1280x720');
      await page.setViewportSize({ width: 1400, height: 900 });
      await sleep(400);
      for (const tab of CONTROL_TABS) {
        await page.click(`[data-tab="control:${tab}"]`);
        await sleep(400);
        await shot(page, `dashboard-${tab}`);
      }
      await page.close();
    }
    {
      const page = await open('#/ab', 500);
      await untilSamples(page, 15000);
      await sleep(3000);
      await shot(page, 'ab');
      await page.close();
    }
    {
      const page = await open('#/showcase', 1500);
      await shot(page, 'showcase');
      await page.close();
    }
    {
      const page = await open('#/showcase?script=rolling-hotspot', 8000);
      await shot(page, 'showcase-rolling-hotspot');
      await page.close();
    }
    {
      const page = await open('?server=off#/dashboard', 500);
      await untilSamples(page, 10000);
      await sleep(1500);
      await shot(page, 'dashboard-replay');
      await page.close();
    }
  } finally {
    await stopRuns();
    await browser.close();
  }
})().catch(e => { console.error('screens: ' + (e && e.stack || e)); process.exit(1); });
