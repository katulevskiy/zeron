import { defineConfig } from "vitest/config";
export default defineConfig({ test: { environment: "node", include: ["tests/project-relay.test.ts", "tests/project-visual.browser.ts"], hookTimeout: 600_000, testTimeout: 120_000, maxWorkers: 1, minWorkers: 1 } });
