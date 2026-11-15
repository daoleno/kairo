// Kairo in Simplified Chinese: the system's language, or the one Settings picks.
import { expect, open, test } from "nsh/test";

const desktop = { platform: "linux", width: 1180, height: 760 } as const;

test("zh: a Chinese system shows Kairo in Chinese", async () => {
  const app = await open("..", { ...desktop, locale: "zh-CN" });
  await app.waitFor("text:首页");
  expect(app.shows("text:设置")).toBe(true);
  expect(app.shows("text:Settings")).toBe(false);
});
