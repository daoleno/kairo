// The web build as a person meets it, in a network namespace of its own (see
// scripts/web-e2e.sh). PKG_DIR: an unpacked Kairo-server tarball. Everything
// (the nodes, the server, its core, Chrome) runs inside the namespace, where
// only loopback exists. PLAYWRIGHT: where playwright-core is; CHROME: Chrome.
const { chromium } = await import(process.env.PLAYWRIGHT || 'playwright-core');
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';

const [,, pkg, out] = process.argv;
fs.mkdirSync(out, { recursive: true });
const work = fs.mkdtempSync(path.join(os.tmpdir(), 'kairo-web-e2e-'));
// A scratch account: install.sh puts the server in its ~/.local as it would
// for a person, and the server finds Kairo's folder where it always does.
const account = path.join(work, 'account');
const home = path.join(account, '.local/share/kairo');
const hits = path.join(work, 'hits');
const PORT = 18200;
const TOKEN = 'e2e-secret-4f1c9a';
const SUB = `http://127.0.0.1:${PORT}/sub?token=${TOKEN}`;
const BASE = 'http://kairo.localhost:7979/';
const PASSWORD = 'correct horse battery staple';
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
const results = [];
const check = (ok, what) => { results.push([ok, what]); log(ok ? 'PASS' : 'FAIL', what); if (!ok) throw new Error(what); };
const sleep = ms => new Promise(r => setTimeout(r, ms));
const procs = [];
const env = { ...process.env, HOME: account, KAIRO_NO_SYSPROXY: '1', KAIRO_NO_UPDATE: '1', KAIRO_NO_ICON_FETCH: '1', RUST_LOG: 'warn' };
// Never this machine's desktop session or service manager, nor its folders.
for (const k of ['DBUS_SESSION_BUS_ADDRESS', 'XDG_RUNTIME_DIR', 'KAIRO_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME']) delete env[k];

function start(cmd, args, opts = {}) {
  const p = spawn(cmd, args, { env, stdio: ['ignore', 'pipe', 'pipe'], ...opts });
  procs.push(p);
  return p;
}
function waitLine(p, re, ms = 60000) {
  return new Promise((res, rej) => {
    let buf = '';
    const t = setTimeout(() => rej(new Error('no ' + re + ' in: ' + buf.slice(-500))), ms);
    const on = d => { buf += d; const m = buf.match(re); if (m) { clearTimeout(t); res(m); } };
    p.stdout.on('data', on); p.stderr.on('data', d => fs.appendFileSync(path.join(out, 'server.log'), d));
  });
}
function count(node) { try { return +fs.readFileSync(path.join(hits, node), 'utf8'); } catch { return 0; } }
function curl(args) {
  try { return execFileSync('curl', ['-sS', '--max-time', '8', ...args], { encoding: 'utf8' }); } catch (e) { return null; }
}
async function startServer() {
  const s = start(path.join(account, '.local/bin/kairo-server'), ['serve', '--listen', '127.0.0.1:7979']);
  const lines = [];
  s.stdout.on('data', d => lines.push(String(d)));
  await waitLine(s, /Kairo is at/);
  return { s, text: () => lines.join('') };
}

let browser, server;
try {
  // The provider, and Kairo's folder (only the delay test's address is set:
  // the namespace has no internet for the default one).
  const nodes = start('python3', ['-I', path.join(path.dirname(new URL(import.meta.url).pathname), 'nodes.py'), String(PORT), hits]);
  await waitLine(nodes, /ready/);
  fs.mkdirSync(account, { recursive: true });
  execFileSync('sh', [path.join(pkg, 'install.sh')], { env, stdio: 'inherit' });
  const unit = fs.readFileSync(path.join(account, '.config/systemd/user/kairo-server.service'), 'utf8');
  check(unit.includes(`ExecStart=${account}/.local/bin/kairo-server serve`) && unit.includes('Restart=always'), 'install.sh: the server in ~/.local and its user service');
  check(fs.existsSync(path.join(account, '.local/share/kairo-server/web/index.html')) && !fs.existsSync(home), 'the page beside Kairo\'s folder, not in it');
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(path.join(home, 'settings.json'), JSON.stringify({ testUrl: `http://127.0.0.1:${PORT + 3}/generate_204`, testTimeoutMs: 2000 }));
  server = await startServer();
  const code = server.text().match(/Setup code: (\S+)/)[1];
  check(!!code, 'the server printed a one-time setup code');

  browser = await chromium.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome-stable', headless: true, args: ['--no-sandbox'] });
  const frames = [];
  const watch = p => {
    // What the server sent (what the page sent includes the link the person typed).
    p.on('websocket', ws => { ws.on('framereceived', f => frames.push(String(f.payload))); });
    p.on('pageerror', e => log('pageerror', String(e).slice(0, 200)));
  };
  const shot = async (p, name) => { await sleep(1200); await p.screenshot({ path: path.join(out, name + '.png') }); };

  // --- A: desktop, light. First run.
  const a = await browser.newContext({ viewport: { width: 1280, height: 820 }, colorScheme: 'light' });
  const pa = await a.newPage(); watch(pa);
  await pa.goto(BASE);
  await pa.getByText(/code Kairo printed/).waitFor({ timeout: 60000 });
  await shot(pa, '01-desktop-setup');
  await pa.getByPlaceholder('Setup code').fill(code);
  await pa.getByPlaceholder('New password', { exact: true }).fill(PASSWORD);
  await pa.getByPlaceholder('New password again').fill(PASSWORD);
  await pa.getByText('Set Password', { exact: true }).click();
  await pa.getByText('Home', { exact: true }).first().waitFor({ timeout: 60000 });
  await shot(pa, '02-desktop-home-first');
  const cookie = (await a.cookies()).find(c => c.name === 'kairo_session');
  check(cookie && cookie.httpOnly && cookie.sameSite === 'Strict' && cookie.domain === 'kairo.localhost', 'signed in: an HttpOnly, Strict cookie for kairo.localhost only');

  // A subscription by link.
  await pa.getByPlaceholder(/subscription link/).fill(SUB);
  await pa.getByRole('button', { name: 'Connect' }).click();
  await pa.getByText('Connected', { exact: true }).first().waitFor({ timeout: 90000 });
  await sleep(4000);
  await shot(pa, '03-desktop-connected');
  const settings = JSON.parse(fs.readFileSync(path.join(home, 'settings.json'), 'utf8'));
  const mixed = settings.mixedPort;
  log('mixed port', mixed);

  // Pick an exit: Everything else → United States, then Japan; each time a
  // real request goes out through the mixed port, and the node that carried
  // it is the one picked.
  const via = () => curl(['-x', `http://127.0.0.1:${mixed}`, `http://203.0.113.10:${PORT}/hello`]);
  for (const [country, server, node, other] of [['United States', 'Seattle 01', 'seattle', 'tokyo'], ['Japan', 'Tokyo 01', 'tokyo', 'seattle']]) {
    await pa.getByText('Exits', { exact: true }).first().click();
    await sleep(800);
    if (!(await pa.getByText(server, { exact: true }).count())) await pa.getByText(country, { exact: true }).last().click();
    await sleep(600);
    await pa.getByText(server, { exact: true }).last().click();
    await pa.getByText(`Chosen · ${server}`).first().waitFor({ timeout: 15000 });
    await sleep(1500);
    await shot(pa, `04-desktop-exit-${node}`);
    const before = [count(node), count(other)];
    const body = via();
    check(body && body.includes('hello'), `a request through the proxy port is answered (${country})`);
    check(count(node) === before[0] + 1 && count(other) === before[1], `it left through the picked server: ${node}`);
  }

  // Every app (TUN) on: a plain request, no proxy setting, goes through the
  // picked server; off: nothing gets out (only loopback is here).
  check(curl([`http://203.0.113.10:${PORT}/hello`]) === null, 'before Every app, a plain request has no way out');
  await pa.getByText('Home', { exact: true }).first().click();
  await sleep(800);
  await pa.getByText('Every app', { exact: true }).first().click();
  let plain = null;
  for (let i = 0; i < 20 && !plain; i++) { await sleep(1000); plain = curl([`http://203.0.113.10:${PORT}/hello`]); }
  await shot(pa, '05-desktop-every-app-on');
  check(plain && plain.includes('hello'), 'with Every app on, a plain request leaves through Kairo');
  const tokyo = count('tokyo');
  curl([`http://203.0.113.10:${PORT}/hello`]);
  check(count('tokyo') === tokyo + 1, 'and through the picked server (tokyo)');
  await pa.getByText('System settings', { exact: true }).first().click();
  let gone = false;
  for (let i = 0; i < 20 && !gone; i++) { await sleep(1000); gone = curl([`http://203.0.113.10:${PORT}/hello`]) === null; }
  await shot(pa, '06-desktop-every-app-off');
  check(gone, 'with Every app off, it is gone again');

  // --- B: a phone, dark. Signs in; shows the pages.
  const b = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2, colorScheme: 'dark' });
  const pb = await b.newPage(); watch(pb);
  await pb.goto(BASE);
  await pb.getByText('Sign in to Kairo').waitFor({ timeout: 60000 });
  await shot(pb, '07-phone-sign-in');
  await pb.getByPlaceholder('Password').fill(PASSWORD);
  await pb.keyboard.press('Enter');
  await pb.getByText('Connected', { exact: true }).first().waitFor({ timeout: 60000 });
  await shot(pb, '08-phone-home');
  await pb.getByText('Exits', { exact: true }).last().click();
  await shot(pb, '09-phone-exits');
  // An exit picked on the phone: the request leaves through that server.
  await pb.getByText('Everything else', { exact: true }).first().click();
  await sleep(800);
  if (!(await pb.getByText('Seattle 01', { exact: true }).count())) await pb.getByText('United States', { exact: true }).last().click();
  await sleep(600);
  await pb.getByText('Seattle 01', { exact: true }).last().click();
  await pb.getByText('Chosen · Seattle 01').first().waitFor({ timeout: 15000 });
  await shot(pb, '09b-phone-exit-seattle');
  const seattle = count('seattle');
  check(via()?.includes('hello') && count('seattle') === seattle + 1, 'picked on the phone, the request leaves through seattle');

  // A signs B out from Settings › Signed-in browsers.
  await pa.getByText('Settings', { exact: true }).first().click();
  await pa.mouse.move(700, 450); await pa.mouse.wheel(0, 700);
  await shot(pa, '10-desktop-signed-in-browsers');
  const outs = pa.getByText('Sign Out', { exact: true });
  check(await outs.count() >= 2, 'Settings lists both browsers');
  await outs.nth(1).click();
  await pb.getByText('Sign in to Kairo').waitFor({ timeout: 15000 });
  await shot(pb, '11-phone-signed-out');
  check(true, 'the phone was signed out at once, by the desktop');

  // The server restarts: A stays signed in, the core comes back.
  server.s.kill('SIGTERM');
  await new Promise(r => server.s.on('exit', r));
  server = await startServer();
  check(!/Setup code/.test(server.text()), 'after a restart no setup code: the password stands');
  await pa.reload();
  await pa.getByText('Connected', { exact: true }).first().waitFor({ timeout: 90000 });
  await shot(pa, '12-desktop-after-restart');
  check(true, 'after a restart the desktop browser is still signed in, and connected');
  check(via()?.includes('hello'), 'and requests go out again');
  await pb.reload();
  await pb.getByText('Sign in to Kairo').waitFor({ timeout: 60000 });
  check(true, 'the revoked phone stays signed out after the restart');

  // Nothing the pages received carried the subscription's token.
  const leaked = frames.filter(f => f.includes(TOKEN));
  if (leaked.length) fs.writeFileSync(path.join(out, 'leaked.txt'), leaked.join('\n\n'));
  check(frames.length > 20 && !leaked.length, `no frame from the server carried the link's token (${frames.length} frames)`);
  const mode = fs.statSync(path.join(home, 'profiles.json')).mode & 0o777;
  check(mode === 0o600, 'the subscription list on disk is its owner\'s alone (0600)');
} catch (e) {
  log('ERROR', e.message);
  process.exitCode = 1;
} finally {
  if (browser) await browser.close().catch(() => {});
  for (const p of procs) { try { p.kill('SIGTERM'); } catch {} }
  await sleep(1500);
  fs.writeFileSync(path.join(out, 'results.json'), JSON.stringify(results, null, 1));
  fs.rmSync(work, { recursive: true, force: true });
  log(results.filter(r => r[0]).length, 'passed,', results.filter(r => !r[0]).length, 'failed');
}
