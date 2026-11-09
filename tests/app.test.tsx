// Kairo headless, against a real core. scripts/ui-test.sh starts the fixture
// (a subscription and proxy nodes on 28080..28087) and points Kairo at a
// scratch home whose core listens on 27897/29097; the system proxy is never
// touched. The tests share the one Kairo service of the runner, in order:
// the subscription added first is what the later ones see.
import { expect, open, test } from "nsh/test";

const SUB = "http://127.0.0.1:28080/sub";
const NOTICE = "http://127.0.0.1:28080/notice";
const desktop = { platform: "linux", width: 1180, height: 760 } as const;
const slow = { timeout: 20000 };

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
  expect(app.texts()).toContain("Match");
  await app.type("text:Type, payload or target", "REJECT");
  await app.waitUntil(() => has(app.texts(), (t) => / of \d+$/.test(t)), slow);
  expect(app.texts()).not.toContain("Match");
});

test("logs: the core's lines, live", async () => {
  const app = await open("..", desktop);
  await app.press("Logs");
  // Debug shows the core's resolver at work: look something up, come back.
  await app.press("text:Debug");
  await app.press("DNS");
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
  await app.press("DNS");
  await app.type("text:example.com", "localhost");
  await app.press("Query");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("localhost  A")), slow);
});

test("settings: an edit waits for Apply, Revert puts it back", async () => {
  const app = await open("..", desktop);
  await app.press("Settings");
  await app.waitFor("text:Controller", slow);
  expect(has(app.texts(), (t) => t.startsWith("mihomo v") || t.startsWith("v1."))).toBe(true);
  expect(app.shows("text:Unapplied changes")).toBe(false);
  expect(app.value("id:mixed-port")).toBe("27897");
  await app.clear("id:mixed-port");
  await app.type("id:mixed-port", "27898");
  await app.waitFor("text:Unapplied changes");
  await app.press("Revert");
  await app.waitUntil(() => !app.shows("text:Unapplied changes"));
  expect(app.value("id:mixed-port")).toBe("27897");
});

test("follows the system's dark appearance", async () => {
  const app = await open("..", { ...desktop, colorScheme: "light" });
  const light = app.spanStyle("text:Home >> nth=0").color;
  await app.setColorScheme("dark");
  const dark = app.spanStyle("text:Home >> nth=0").color;
  expect(dark).not.toBe(light);
});
