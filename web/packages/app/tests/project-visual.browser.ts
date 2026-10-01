import { access, mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { chromium } from "playwright";
import { expect, test } from "vitest";
import { startBrowserRelayFixture } from "../../engine-client/tests/helpers/browser-relay";

// Real built app + native browser sockets + actual cookie edge and two EngineCores.
test("desktop and phone browse, cancel, missing path and explicit selected-engine creation", async () => {
  const repo = resolve("../../..");
  const screenshots = join(repo, ".scratch/pr526-unslop/ticket10-visual");
  await mkdir(screenshots, { recursive: true });
  const fixture = await startBrowserRelayFixture({ assetsDirectory: resolve("dist") });
  const browser = await chromium.launch({ headless: true });
  try {
    for (const [label, viewport] of [["desktop", { width: 1440, height: 1000 }], ["phone", { width: 390, height: 844 }]] as const) {
      const context = await browser.newContext({ viewport });
      await context.addCookies(fixture.session.browserCookies());
      const page = await context.newPage();
      const errors: string[] = [];
      page.on("pageerror", error => errors.push(error.message));
      await page.goto(fixture.origin);
      await page.locator(".main-inner").waitFor({ timeout: 20_000 });
      await page.getByText(`Chat on ${fixture.engines[0]!.label}`, { exact: true }).first().waitFor({ state: "attached", timeout: 20_000 });
      await page.keyboard.press("Control+Shift+N");
      await page.getByPlaceholder("Search devices…").waitFor();
      await page.getByPlaceholder("Search devices…").fill(fixture.engines[1]!.label);
      await page.getByPlaceholder("Search devices…").press("Enter");
      await page.getByPlaceholder("Search locations…").waitFor();
      await page.getByText("Home", { exact: true }).last().click();
      const search = page.getByPlaceholder("Search folders…");
      await search.waitFor();
      await search.fill(fixture.engines[1]!.projectRoot);
      await expect.poll(async () => page.getByRole("button", { name: "Add project" }).isEnabled()).toBe(true);
      await page.screenshot({ path: join(screenshots, `${label}-browse.png`) });
      const card = await page.locator(".add-space-card").boundingBox();
      expect(card).not.toBeNull();
      expect(card!.x).toBeGreaterThanOrEqual(0);
      expect(card!.x + card!.width).toBeLessThanOrEqual(viewport.width);
      expect(card!.y).toBeGreaterThanOrEqual(0);
      expect(card!.y + card!.height).toBeLessThanOrEqual(viewport.height);
      const missing = `${fixture.engines[1]!.projectRoot}/typed-missing-${label}`;
      await search.fill(missing);
      await page.getByRole("alert").waitFor();
      await page.screenshot({ path: join(screenshots, `${label}-missing.png`) });
      await expect(access(missing)).rejects.toThrow(/ENOENT/);
      expect(await page.getByRole("alert").textContent()).not.toMatch(/unknown method/i);
      await page.keyboard.press("Escape");
      await page.getByPlaceholder("Search folders…").waitFor({ state: "hidden" });
      await page.keyboard.press("Control+Shift+N");
      await page.getByPlaceholder("Search devices…").fill(fixture.engines[1]!.label);
      await page.getByPlaceholder("Search devices…").press("Enter");
      await page.getByText("Home", { exact: true }).last().click();
      await page.getByLabel("New repository name").fill(`ticket10-${label}`);
      await page.screenshot({ path: join(screenshots, `${label}-explicit.png`) });
      for (const selector of [".add-space-card", ".add-space-footer", ".add-space-footer button", ".add-space-card form button"]) {
        const bounds = await page.locator(selector).boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport.width);
        expect(bounds!.y).toBeGreaterThanOrEqual(0);
        expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height);
      }
      await page.getByRole("button", { name: "Create repository and add" }).click();
      await page.getByLabel("New repository name").waitFor({ state: "hidden", timeout: 15_000 });
      await page.getByText(`ticket10-${label}`, { exact: true }).first().waitFor({ timeout: 15_000 });
      await page.screenshot({ path: join(screenshots, `${label}-created.png`) });
      const created = join(dirname(fixture.engines[1]!.projectRoot), "repos", `ticket10-${label}`);
      await access(join(created, ".git"));
      await expect(access(join(dirname(fixture.engines[0]!.projectRoot), "repos", `ticket10-${label}`))).rejects.toThrow(/ENOENT/);
      expect(errors).toEqual([]);
      console.log(`${label} rendered creation on ${created}`);
      await context.close();
    }
  } catch (error) {
    for (const [index, context] of browser.contexts().entries()) {
      const page = context.pages()[0];
      if (page) {
        await page.screenshot({ path: join(screenshots, `failure-${index}.png`) });
        await writeFile(join(screenshots, `failure-${index}.html`), await page.content());
      }
    }
    throw error;
  } finally { await browser.close(); await fixture.stop(); }
});
