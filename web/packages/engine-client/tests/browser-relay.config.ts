import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  root: resolve(dirname(fileURLToPath(import.meta.url)), ".."),
  test: {
    environment: "node",
    include: ["tests/browser-relay.test.ts"],
    hookTimeout: 600_000,
    testTimeout: 120_000,
    maxWorkers: 2,
    minWorkers: 1,
  },
});
