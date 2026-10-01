import { createRequire } from "node:module";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { afterAll, beforeAll, expect, test } from "vitest";
import { startBrowserRelayFixture } from "./helpers/browser-relay";

// Parent owns shared manifests. Resolve the installed Playwright through
// NODE_PATH when it is not a local workspace dependency; no replacement UI.
const { chromium } = createRequire(import.meta.url)("playwright");
const evidence = resolve("../../../.scratch/pr526-unslop/evidence/sidebar");
let fixture;
let browser;
const errors = [];
const failedResponses = [];
const pages = [];
beforeAll(async () => {
  await mkdir(evidence, { recursive: true });
  fixture = await startBrowserRelayFixture({ assetsDirectory: resolve("../app/dist") });
  browser = await chromium.launch({ headless: true });
});
afterAll(async () => {
  try {
    for (const [index, page] of pages.entries()) {
      await page.screenshot({ path: resolve(evidence, `last-viewport-${index}.png`) });
      await writeFile(resolve(evidence, `last-viewport-${index}.html`), await page.content());
    }
    await writeFile(resolve(evidence, "page-errors.json"), JSON.stringify({ errors, failedResponses }, null, 2));
  } finally { await browser?.close(); await fixture?.stop(); }
});

async function viewport(width, height) {
  const context = await browser.newContext({ viewport: { width, height } });
  await context.addCookies(fixture.session.browserCookies());
  const page = await context.newPage();
  pages.push(page);
  page.on("console", message => { if (message.type() === "error") errors.push(`console: ${message.text()}`); });
  page.on("response", response => { if (response.status() >= 400) failedResponses.push({ status: response.status(), path: new URL(response.url()).pathname }); });
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(fixture.origin);
  await page.locator(".chat-row-item").first().waitFor({ state: "attached", timeout: 20000 });
  const row = page.locator(".chat-row-item").first();
  const inViewport = await row.evaluate(e => { const r = e.getBoundingClientRect(); return r.right > 0 && r.left < innerWidth; });
  if (!inViewport) await page.getByRole("button", { name: "Toggle sidebar", exact: true }).click();
  await page.waitForFunction(() => { const r = document.querySelector(".sidebar").getBoundingClientRect(); return r.left >= -1 && r.right > 100; }, null, { timeout: 15000 });
  await row.waitFor({ state: "visible" });
  await page.waitForFunction(() => ![...document.querySelectorAll('[role="status"]')].some(e => /Waiting for workspace sidebar/.test(e.textContent)));
  return page;
}
async function pin(page, selector = ".chat-row-item") {
  await page.locator(selector).first().click({ button: "right" });
  await page.getByRole("button", { name: "Pin", exact: true }).click();
  await page.locator(".pinned-row").first().waitFor({ state: "visible" });
}
async function snapshot(page, name) {
  // Capture the viewport-sized shell only after its width transition settles.
  await page.waitForFunction(() => {
    const e = document.querySelector(".sidebar");
    const r = e.getBoundingClientRect();
    const target = innerWidth < 768 ? Math.min(320, innerWidth * 0.85) : parseFloat(getComputedStyle(e).getPropertyValue("--rb-sidebar-now"));
    return Math.abs(r.width - target) < 1;
  }, null, { timeout: 15000 });
  await page.locator(".sidebar").evaluate(async e => {
    await Promise.all(e.getAnimations({ subtree: true })
      .filter(a => Number.isFinite(a.effect?.getTiming().iterations ?? 1))
      .map(a => a.finished.catch(() => {})));
  });
  await page.screenshot({ path: resolve(evidence, `${name}.png`) });
  return page.evaluate(() => ({
    viewport: { width: innerWidth, height: innerHeight },
    documentWidth: document.documentElement.scrollWidth,
    scroll: { x: scrollX, y: scrollY },
    sidebar: (() => { const e = document.querySelector(".sidebar"); const r = e.getBoundingClientRect(); return { x: r.x, width: r.width, transform: getComputedStyle(e).transform, target: getComputedStyle(e).getPropertyValue("--rb-sidebar-now") }; })(),
    pins: [...document.querySelectorAll(".pinned-row .chat-row")].map(e => ({ href: e.getAttribute("href"), text: e.textContent })),
    sections: [...document.querySelectorAll(".sidebar-custom-section")].map(e => ({ id: e.dataset.sidebarSectionId, text: e.textContent })),
    statuses: [...document.querySelectorAll('[role="status"]')].map(e => e.textContent),
  }));
}

test("actual mounted sidebar pin/section flow synchronizes desktop and phone cookie sessions", async () => {
  const desktop = await viewport(1440, 900);
  const phone = await viewport(390, 844);
  await pin(desktop);
  await phone.locator(".pinned-row").first().waitFor({ state: "visible", timeout: 20000 });
  // The view menu and dialog are the actual production controls.
  await desktop.getByRole("button", { name: "Sidebar view options", exact: true }).click();
  await desktop.getByRole("menuitem", { name: "Create Section", exact: true }).click();
  await desktop.getByRole("textbox", { name: "Section name", exact: true }).fill("Focus across viewports");
  await desktop.getByRole("button", { name: "Create section", exact: true }).click();
  await desktop.locator(".sidebar-custom-section").filter({ hasText: "Focus across viewports" }).waitFor({ state: "visible" });
  await phone.locator(".sidebar-custom-section").filter({ hasText: "Focus across viewports" }).waitFor({ state: "visible", timeout: 20000 });
  const states = { desktop: await snapshot(desktop, "desktop-pinned-section"), phone: await snapshot(phone, "phone-pinned-section") };
  await writeFile(resolve(evidence, "rendered-states.json"), JSON.stringify({ states, pageErrors: errors }, null, 2));
  expect(states.desktop.pins.length).toBeGreaterThan(0);
  expect(states.phone.pins.length).toBeGreaterThan(0);
  expect(states.desktop.sections.length).toBeGreaterThan(0);
  expect(states.phone.sections.length).toBeGreaterThan(0);
  expect(states.phone.documentWidth).toBeLessThanOrEqual(390);
  // An unpin through the phone's production context menu appears on desktop.
  await phone.locator(".pinned-row .chat-row-item").first().click({ button: "right" });
  await phone.getByRole("button", { name: "Unpin", exact: true }).click();
  await desktop.waitForFunction(() => document.querySelectorAll(".pinned-row").length === 0);
  await phone.waitForFunction(() => document.querySelectorAll(".pinned-row").length === 0);
  states.desktopAfterUnpin = await snapshot(desktop, "desktop-unpinned-section");
  states.phoneAfterUnpin = await snapshot(phone, "phone-unpinned-section");
  await writeFile(resolve(evidence, "rendered-states.json"), JSON.stringify({ states, pageErrors: errors, failedResponses }, null, 2));
  expect(errors).toEqual([]);
});
