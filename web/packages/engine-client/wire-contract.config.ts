import { defineConfig } from "vitest/config";

// Separate from advanced browser feature suites: tickets 10/11 are not core gates.
export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/wire-contract.test.ts"],
    hookTimeout: 600_000,
    testTimeout: 30_000,
  },
});
