// Kairo headless, against a real core. scripts/ui-test.sh starts the fixture
// (a subscription and proxy nodes on 28080..28087) and points Kairo at a
// scratch home whose core listens on 27897/29097; the system proxy is never
// touched. The tests share the one Kairo service of the runner, in order:
// the profile added first is what the later ones see.
import { expect, open, test } from "nsh/test";

const SUB = "http://127.0.0.1:28080/sub";
const desktop = { platform: "linux", width: 1180, height: 760 } as const;
const slow = { timeout: 20000 };

const has = (texts: string[], pred: (t: string) => boolean) => texts.some(pred);

test("a first run: the core, no profile yet", async () => {
  const app = await open("..", desktop);
  await app.waitUntil(() => !app.shows("text:Core not installed"), slow);
  expect(app.texts()).toContain("No profile yet. Add a subscription URL or a config file.");
  expect(app.shows("text:Add Profile…")).toBe(true);
});

test("adding a subscription makes it active and starts the core", async () => {
  const app = await open("..", desktop);
  await app.press("text:Add Profile… >> nth=0");
  await app.waitFor("text:Profiles");
  await app.press("Add Profile >> nth=0");
  await app.type("text:https://example.com/subscribe?token=…", SUB);
  await app.press("text:Add");
  await app.waitFor("text:Demo Cloud", slow);
  expect(app.shows("text:Active")).toBe(true);
  // Usage from the subscription-userinfo header.
  await app.waitUntil(() => has(app.texts(), (t) => t.includes("of 200 GB")), slow);

  await app.press("Overview");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Running · mihomo")), slow);
  expect(app.texts()).toContain("Demo Cloud");
});

test("proxies: groups in config order, a node picked, delays tested", async () => {
  const app = await open("..", desktop);
  await app.press("Proxies");
  await app.waitFor("text:Fallback", slow);
  const texts = app.texts();
  for (const g of ["Proxy", "Auto", "Asia", "United States", "Streaming", "Final"]) {
    expect(texts).toContain(g);
  }
  await app.press("text:Asia >> nth=0");
  await app.waitFor("text:Test All");
  await app.press("Test All");
  await app.waitUntil(() => has(app.texts(), (t) => /^\d+ ms$/.test(t)), slow);
  // The dead node (the fixture's port that answers nothing) times out.
  await app.waitUntil(() => !app.shows("text:Testing…"), slow);
});

test("modes switch from the overview and show in the sidebar", async () => {
  const app = await open("..", desktop);
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Running · mihomo")), slow);
  await app.press("text:Global");
  await app.waitUntil(() => app.texts().filter((t) => t === "Global").length >= 2, slow);
  await app.press("text:Direct");
  await app.waitUntil(() => app.texts().filter((t) => t === "Direct").length >= 2, slow);
  await app.press("text:Rule");
  await app.waitUntil(() => app.texts().filter((t) => t === "Rule").length >= 2, slow);
});

test("rules: the running configuration's, searchable", async () => {
  const app = await open("..", desktop);
  await app.press("Rules");
  await app.waitUntil(() => has(app.texts(), (t) => t.startsWith("Rules  ")), slow);
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
  const light = app.spanStyle("text:Overview >> nth=1").color;
  await app.setColorScheme("dark");
  const dark = app.spanStyle("text:Overview >> nth=1").color;
  expect(dark).not.toBe(light);
});
