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
  // Home: three exits, each with a place and how it is doing; the plan from the provider's header.
  await app.waitUntil(() => has(app.texts(), (t) => /^Fastest · \S/.test(t)), slow);
  expect(app.texts()).toContain("AI");
  expect(app.texts()).toContain("Everything else");
  expect(app.texts()).toContain("Downloads");
  expect(has(app.texts(), (t) => t.startsWith("Demo Cloud · ") && t.includes("of 200 GB"))).toBe(true);
  expect(app.shows("text:Apps & Sites")).toBe(true);
  // The tests are not on Home any more: no speed graph, no milliseconds.
  expect(has(app.texts(), (t) => /^\d+ ms$/.test(t))).toBe(false);
});

test("exits: a place of one's own for everything else, then Fastest again", async () => {
  const app = await open("..", desktop);
  await app.waitFor("text:Connected", slow);
  await app.press("Exits");
  await app.waitFor("text:Where traffic leaves", slow);
  // The tiles keep to a word; the servers' delays are in the picker, under each country.
  await app.press("text:Everything else >> nth=0");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ ms$/.test(t)), slow);
  await app.press("text:All servers");
  await app.waitFor("text:Tokyo 02", slow);
  await app.press("text:Tokyo 02");
  await app.waitUntil(() => has(app.texts(), (t) => t === "Chosen · Tokyo 02"), slow);
  await app.press("text:Fastest");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Fastest · ")), slow);
  // Quality is under the servers; with nothing recorded yet it says so.
  expect(app.shows("text:Quality")).toBe(true);
  await app.waitFor("text:Collecting data", slow);
});

test("mode: Smart, Everything, Direct, one press each, and Kairo stays connected", async () => {
  const app = await open("..", desktop);
  await app.press("Home");
  await app.waitFor("text:Mode", slow);
  for (const word of ["Everything", "Direct", "Smart"]) {
    await app.press(`text:${word} >> nth=0`);
    await app.wait(700);
    expect(app.shows("text:Connected")).toBe(true);
    // The three stay where they were: the choice is a mark, not a new page.
    expect(app.shows("text:Mode")).toBe(true);
  }
});

test("connect: off and on again, and two quick presses end where the last one points", async () => {
  const app = await open("..", desktop);
  await app.press("Home");
  await app.waitFor("text:Connected", slow);
  await app.press("Connect");
  await app.waitFor("text:Not Connected", slow);
  await app.press("Connect");
  await app.waitFor("text:Connected", slow);
  // Off, then on again within a breath, before the screen has caught up: it is on.
  await app.press("Connect");
  await app.wait(130);
  await app.press("Connect");
  await app.wait(3000);
  await app.waitFor("text:Connected", slow);
  expect(app.shows("text:Not Connected")).toBe(false);
});

test("apps & sites: an app and a site of one's own, each removed by its own button", async () => {
  const app = await open("..", desktop);
  await app.press("Apps & Sites");
  await app.waitFor("text:Nothing set apart", slow);
  await app.press("Add Site");
  await app.waitFor("text:Add a site", slow);
  await app.type("text:example.com", "kairo-sites.test");
  await app.press("Add");
  await app.waitFor("text:kairo-sites.test", slow);
  expect(has(app.texts(), (t) => /^1 choices?$/.test(t))).toBe(true);
  expect(app.shows("text:Nothing set apart")).toBe(false);
  await app.press("Add App");
  await app.waitFor("text:Add an app", slow);
  await app.type("text:Telegram", "kairo-app");
  await app.press("Add");
  await app.waitFor("text:kairo-app", slow);
  expect(has(app.texts(), (t) => /^2 choices$/.test(t))).toBe(true);
  // Each row has its own way out.
  await app.press("Remove kairo-sites.test");
  await app.waitUntil(() => !app.shows("text:kairo-sites.test"), slow);
  expect(app.shows("text:kairo-app")).toBe(true);
  await app.press("Remove kairo-app");
  await app.waitFor("text:Nothing set apart", slow);
  expect(app.shows("text:kairo-app")).toBe(false);
});

test("groups: the subscription's own, under Settings, with a way back", async () => {
  const app = await open("..", desktop);
  await goTo(app, "Groups");
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

test("rules: the running configuration's, searchable; the AI layer's come with their group", async () => {
  const app = await open("..", desktop);
  await goTo(app, "Rules");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ rules$/.test(t)), slow);
  await app.type("text:Filter rules", "claude");
  await app.waitUntil(() => app.texts().includes("claude.ai"), slow);
  expect(app.texts()).toContain("Kairo AI");
  expect(has(app.texts(), (t) => / of \d+$/.test(t))).toBe(true);
});

test("rules: a filter narrows to what it matches", async () => {
  const app = await open("..", desktop);
  await goTo(app, "Rules");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ rules$/.test(t)), slow);
  await app.type("text:Filter rules", "MATCH");
  await app.waitUntil(() => has(app.texts(), (t) => / of \d+$/.test(t)), slow);
  expect(app.texts()).toContain("Match");
  expect(app.texts()).not.toContain("claude.ai");
});

test("logs: the core's lines, live", async () => {
  const app = await open("..", desktop);
  await goTo(app, "Diagnostics");
  // Debug shows the core's resolver at work: look something up, come back.
  await app.press("text:Debug");
  await goTo(app, "DNS");
  await app.type("text:example.com", "kairo-logs.test");
  await app.press("Query");
  await goTo(app, "Diagnostics");
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

test("AI: its exit beside the others, the flow on Activity, the switches in Settings, no promises", async () => {
  const app = await open("..", desktop);
  const noPromises = () => {
    for (const t of app.texts()) {
      expect(/never (be )?(flagged|banned)|unlock|clean IP|safe/i.test(t)).toBe(false);
    }
  };
  await app.press("Exits");
  await app.waitFor("text:Where traffic leaves", slow);
  // AI is an exit of its own: where it goes, and whether it stays when the Location changes.
  await app.waitUntil(() => has(app.texts(), (t) => /· (follows Everything else|pinned )/.test(t)), slow);
  expect(app.texts()).toContain("AI");
  noPromises();
  await app.press("Home");
  noPromises();
  // Activity: by app, with what each talks to and where it goes through.
  await app.press("Activity");
  await app.waitFor("text:By App", slow);
  expect(app.shows("text:All Connections")).toBe(true);
  for (const h of ["App", "Talks to", "Goes through", "Total"]) {
    expect(app.texts()).toContain(h);
  }
  // Statistics: a fresh home has an hour to wait, and says so instead of drawing an empty chart.
  expect(app.shows("text:Today")).toBe(true);
  await app.waitFor("text:Collecting data", slow);
  expect(app.shows("text:Clear History")).toBe(false);
  await app.press("text:7 days");
  await app.waitFor("text:Collecting data", slow);
  noPromises();
  await app.press("Settings");
  await app.waitFor("text:AI tools use one exit", slow);
  expect(app.shows("text:Domestic AI goes direct")).toBe(true);
  noPromises();
  // What almost nobody changes is behind Advanced; what was cut is gone.
  expect(app.shows("text:Port (HTTP + SOCKS)")).toBe(false);
  expect(app.shows("text:TUN stack")).toBe(false);
  expect(app.shows("text:Log level")).toBe(false);
  await app.press("text:Advanced");
  await app.waitFor("text:Port (HTTP + SOCKS)", slow);
  expect(app.shows("text:The subscription's groups")).toBe(true);
  expect(app.shows("text:Diagnostics")).toBe(true);
  await app.press("text:General");
  await app.waitFor("text:AI tools use one exit", slow);
});

test("settings: an edit waits for Apply, Revert puts it back", async () => {
  const app = await open("..", desktop);
  await app.press("Settings");
  await app.waitFor("text:Appearance", slow);
  await app.press("text:Advanced");
  await app.waitFor("text:Controller", slow);
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
