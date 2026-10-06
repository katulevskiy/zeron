import { defineConfig } from "@playwright/test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
export default defineConfig({
  testDir: "./test",
  testMatch: "**/*.spec.mjs",
  workers: 1,
  timeout: 90_000,
  expect: { timeout: 15_000 },
  forbidOnly: Boolean(process.env.CI),
  use: {
    baseURL: "http://localhost:3091",
    viewport: { width: 1440, height: 1000 },
    channel: process.env.REVIEW_BROWSER_CHANNEL || undefined,
    trace: "retain-on-failure",
  },
  webServer: {
    command: `${process.env.REVIEW_TEST_BINARY || "target/release/zeron-review"} --demo`,
    url: "http://localhost:3091/health",
    reuseExistingServer: false,
    env: {
      PORT: "3091",
      PUBLIC_URL: "http://localhost:3091",
      DATA_PATH: join(
        mkdtempSync(join(tmpdir(), "zeron-review-test-")),
        "review.sqlite",
      ),
    },
  },
});
