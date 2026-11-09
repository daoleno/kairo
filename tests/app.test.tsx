// Kairo headless, against a real core. scripts/ui-test.sh starts the fixture
// (a subscription and proxy nodes on a block of free ports) and points Kairo at a
// scratch home whose core listens on free ports too (tests/ports.ts); the system proxy is never
// touched. The tests share the one Kairo service of the runner, in order:
// the subscription added first is what the later ones see.
import { expect, open, test } from "nsh/test";
import { FIXTURE, MIXED } from "./ports";

const SUB = `http://127.0.0.1:${FIXTURE}/sub`;
const NOTICE = `http://127.0.0.1:${FIXTURE}/notice`;
const desktop = { platform: "linux", width: 1180, height: 760 } as const;
const slow = { timeout: 20000 };

// The pages the sidebar does not list are a search away.
const goTo = async (app: any, page: string) => {
  await app.keyboard.press("mod+k");
  await app.type("text:Search Kairo: pages, actions, locations", page);
  await app.press(`text:Go to ${page}`);
};

const has = (texts: string[], pred: (t: string) => boolean) => texts.some(pred);

test("a first run: a welcome that asks only for the link", async () => {
  const app = await open("..", desktop);
  await app.waitFor("text:Welcome to Kairo", slow);
  expect(app.shows("text:Install Core")).toBe(false);
  expect(app.shows("text:Mixed port")).toBe(false);
  expect(app.shows("text:Connect")).toBe(true);
});

test("a provider's notice instead of a config: its own words and Try Again, until fixed", async () => {
  const app = await open("..", desktop);
  await app.type("text:https://… (your provider's subscription link)", NOTICE);
  await app.press("text:Connect");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Your provider says:") && t.includes("开启订阅获取")), slow);
  expect(app.shows("text:Try Again")).toBe(true);
  // It stays (no toast that vanishes).
  await app.wait(7000);
  expect(has(app.texts(), (t) => t.startsWith("Your provider says:"))).toBe(true);
  expect(app.shows("text:Welcome to Kairo")).toBe(true);
});

test("the link in, connected out: the fastest location picked by itself", async () => {
  const app = await open("..", desktop);
  await app.clear("text:https://… (your provider's subscription link)");
  await app.type("text:https://… (your provider's subscription link)", SUB);
  await app.press("text:Connect");
  await app.waitFor("text:Connected", slow);
  // Automatic: a place with its delay, and the plan from the provider's header.
  await app.waitUntil(() => has(app.texts(), (t) => / · \d+ ms · automatic$/.test(t)), slow);
  expect(has(app.texts(), (t) => t.startsWith("Demo Cloud · ") && t.includes("of 200 GB"))).toBe(true);
  expect(app.shows("text:Apps & Sites")).toBe(true);
});

test("location: a place of one's own, then Fastest again", async () => {
  const app = await open("..", desktop);
  await app.waitFor("text:Connected", slow);
  await app.press("text:Location");
  await app.waitFor("text:Search locations");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ ms$/.test(t)), slow);
  await app.type("text:Search locations", "Seattle");
  await app.press("🇺🇸 Seattle 02");
  await app.waitFor("text:Chosen by you", slow);
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("🇺🇸 Seattle 02")), slow);
  await app.press("text:Location");
  await app.press("text:Fastest");
  await app.waitFor("text:Fastest, automatically", slow);
});

test("route: Smart, All, Direct in plain words", async () => {
  const app = await open("..", desktop);
  await app.waitFor("text:Connected", slow);
  await app.press("text:All");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Everything goes through")), slow);
  await app.press("text:Direct");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Everything goes direct")), slow);
  await app.press("text:Smart");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Local sites go direct")), slow);
});

test("apps & sites: the subscription's per-app choices", async () => {
  const app = await open("..", desktop);
  await app.press("Apps & Sites");
  await app.waitFor("text:Streaming", slow);
  expect(app.texts()).toContain("AI");
  expect(app.texts()).toContain("Same as Location");
});

test("connect switch: off and on again", async () => {
  const app = await open("..", desktop);
  await app.press("Home");
  await app.waitFor("text:Connected", slow);
  await app.press("Connect");
  await app.waitFor("text:Not Connected", slow);
  await app.press("Connect");
  await app.waitFor("text:Connected", slow);
});

test("groups: every group in config order, delays tested", async () => {
  const app = await open("..", desktop);
  await app.press("Groups");
  await app.waitFor("text:Fallback", slow);
  const texts = app.texts();
  for (const g of ["Proxy", "Auto", "Asia", "United States", "Streaming", "Final"]) {
    expect(texts).toContain(g);
  }
  await app.press("text:Asia >> nth=0");
  await app.waitFor("text:Test All");
  await app.press("Test All");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ ms$/.test(t)), slow);
  await app.waitUntil(() => !app.shows("text:Testing…"), slow);
});

test("rules: the running configuration's, searchable", async () => {
  const app = await open("..", desktop);
  await app.press("Rules");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ rules$/.test(t)), slow);
  // The layer for AI tools comes first: its rules show with their group.
  expect(app.texts()).toContain("claude.ai");
  await app.type("text:Type, payload or target", "MATCH");
  await app.waitUntil(() => has(app.texts(), (t) => / of \d+$/.test(t)), slow);
  expect(app.texts()).toContain("Match");
  expect(app.texts()).not.toContain("claude.ai");
});

test("logs: the core's lines, live", async () => {
  const app = await open("..", desktop);
  await app.press("Logs");
  // Debug shows the core's resolver at work: look something up, come back.
  await app.press("text:Debug");
  await goTo(app, "DNS");
  await app.type("text:example.com", "kairo-logs.test");
  await app.press("Query");
  await app.press("Logs");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ lines$/.test(t) && t !== "0 lines"), slow);
  await app.press("Pause");
  await app.waitUntil(() => has(app.texts(), (t) => t.endsWith("lines · paused")));
  await app.press("Resume");
  await app.press("text:Info");
});

test("DNS: a lookup through the core's resolver", async () => {
  const app = await open("..", desktop);
  await goTo(app, "DNS");
  await app.type("text:example.com", "localhost");
  await app.press("Query");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("localhost  A")), slow);
});

test("AI: a tile for each service on Home, the flow on Activity, the switches in Settings", async () => {
  const app = await open("..", desktop);
  await app.press("Home");
  await app.waitFor("text:AI", slow);
  for (const name of ["Claude", "ChatGPT", "Gemini", "Cursor", "Copilot"]) {
    await app.waitFor(`text:${name}`, slow);
  }
  // What stays and what goes direct are said plainly.
  expect(app.shows("text:Stays on this computer")).toBe(true);
  expect(app.shows("text:Goes direct")).toBe(true);
  // A tile opens its detail, with the note that the service's terms still apply.
  await app.press("text:Claude");
  await app.waitFor("text:Kairo shows what the service sees. Where you are, and the service's own terms, still apply.", slow);
  expect(app.shows("text:A connection held open")).toBe(true);
  await app.press("text:Claude");
  // Nothing in the app promises anything.
  for (const t of app.texts()) {
    expect(/never (be )?(flagged|banned)|unlock|clean IP|safe/i.test(t)).toBe(false);
  }
  // Activity: nothing yet is an invitation, not an empty chart.
  await app.press("Activity");
  await app.waitUntil(() => app.shows("text:No AI traffic yet") || app.shows("text:Which tools talk to which AI services, since Kairo opened."), slow);
  await app.press("Settings");
  await app.waitFor("text:AI tools use one exit", slow);
  expect(app.shows("text:Domestic AI goes direct")).toBe(true);
  expect(app.shows("text:Downloads follow the Location")).toBe(true);
  // What almost nobody changes is behind Advanced; what was cut is gone.
  expect(app.shows("text:Proxy port")).toBe(false);
  expect(app.shows("text:Start the core with Kairo")).toBe(false);
  expect(app.shows("text:Locations from")).toBe(false);
  await app.press("text:Advanced");
  await app.waitFor("text:Proxy port", slow);
  expect(app.shows("text:Locations from")).toBe(true);
  expect(app.shows("text:Secret")).toBe(true);
  await app.press("text:Advanced");
});

test("settings: an edit waits for Apply, Revert puts it back", async () => {
  const app = await open("..", desktop);
  await app.press("Settings");
  await app.waitFor("text:TUN Mode", slow);
  await app.press("text:Advanced");
  await app.waitFor("text:Controller port", slow);
  expect(has(app.texts(), (t) => t.startsWith("mihomo v") || t.startsWith("v1."))).toBe(true);
  expect(app.shows("text:Unapplied changes")).toBe(false);
  expect(app.value("id:mixed-port")).toBe(String(MIXED));
  await app.clear("id:mixed-port");
  await app.type("id:mixed-port", "27898");
  await app.waitFor("text:Unapplied changes");
  await app.press("Revert");
  await app.waitUntil(() => !app.shows("text:Unapplied changes"));
  expect(app.value("id:mixed-port")).toBe(String(MIXED));
});

test("follows the system's dark appearance", async () => {
  const app = await open("..", { ...desktop, colorScheme: "light" });
  const light = app.spanStyle("text:Home >> nth=0").color;
  await app.setColorScheme("dark");
  const dark = app.spanStyle("text:Home >> nth=0").color;
  expect(dark).not.toBe(light);
});
