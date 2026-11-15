// Kairo in Simplified Chinese: the system's language, or the one Settings picks.
import { expect, open, test } from "nsh/test";

const desktop = { platform: "linux", width: 1180, height: 760 } as const;

test("zh: a Chinese system shows Kairo in Chinese", async () => {
  const app = await open("..", { ...desktop, locale: "zh-CN" });
  await app.waitFor("text:首页");
  expect(app.shows("text:设置")).toBe(true);
  expect(app.shows("text:Settings")).toBe(false);
});

test("zh: a Singapore Chinese system reads the Chinese too", async () => {
  const app = await open("..", { ...desktop, locale: "zh-Hans-SG" });
  await app.waitFor("text:首页");
});

test("zh: Settings picks 简体中文 on an English system, at once, and System gives English back", async () => {
  const app = await open("..", desktop);
  await app.press("text:Settings");
  await app.waitFor("text:Language");
  await app.press("text:简体中文");
  await app.waitFor("text:语言");
  expect(app.shows("text:首页")).toBe(true);
  expect(app.shows("text:外观")).toBe(true);
  expect(app.shows("text:Appearance")).toBe(false);
  // Appearance has a 跟随系统 too: the second is the language's.
  await app.press("text:跟随系统 >> nth=1");
  await app.waitFor("text:Appearance");
  expect(app.shows("text:Home")).toBe(true);
});
