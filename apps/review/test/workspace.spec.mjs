import { test, expect } from "@playwright/test";
test("queue searches, filters, links to details, and renders without script errors", async ({
  page,
}, testInfo) => {
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "Review queue", exact: true }),
  ).toBeVisible();
  await expect(page.locator(".queue-row")).toHaveCount(6);
  await page.evaluate(() => document.fonts.ready);
  expect(
    await page
      .locator(".mark")
      .evaluate((image) => image.complete && image.naturalWidth > 0),
  ).toBe(true);
  await expect(page.locator("body")).toHaveCSS("color", "rgb(236, 231, 247)");
  expect(await page.evaluate(() => document.fonts.check("14px Geist"))).toBe(
    true,
  );
  await page.screenshot({
    path: testInfo.outputPath("review-queue.png"),
    fullPage: true,
  });
  await page.getByLabel("Search changes").fill("Windows");
  await expect(page.locator(".queue-row")).toHaveCount(1);
  await page.locator(".queue-row").click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page).toHaveURL(/item=pr%3A90005/);
  await page.getByRole("button", { name: "Close detail" }).click();
  await page.getByLabel("Search changes").fill("");
  await page.getByLabel("Filter by area").selectOption("sync");
  await expect(page.locator(".queue-row")).toHaveCount(1);
  expect(errors).toEqual([]);
});
test("a reviewer claims work and submits revision-specific review through the UI", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90002");
  await page
    .getByLabel("Demo identity in detail", { exact: true })
    .selectOption("demo-reviewer");
  await expect(
    page.getByLabel("Demo identity in detail", { exact: true }),
  ).toHaveValue("demo-reviewer");
  await page
    .locator(".task")
    .filter({ hasText: "code review" })
    .getByRole("button", { name: "Claim task" })
    .click();
  await expect(
    page.locator(".task").filter({ hasText: "code review" }),
  ).toContainText("demo-reviewer");
  await page.getByText("Submit a code review", { exact: true }).click();
  const form = page.locator('form[data-action="review"]');
  await form
    .getByLabel("Findings and scope reviewed")
    .fill("Checked dropped path normalization and regression coverage.");
  await form.getByRole("button", { name: "Submit review" }).click();
  await expect(page.locator(".drawer > .tag")).toHaveText("needs validation");
  await page.reload();
  await expect(page.locator(".drawer")).toContainText(
    "Checked dropped path normalization",
  );
});
test("a validator completes the review packet and authors cannot self-verify", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90004");
  await page
    .getByLabel("Demo identity in detail", { exact: true })
    .selectOption("demo-validator");
  await page.getByText("Record behavior validation", { exact: true }).click();
  const form = page.locator('form[data-action="validate"]');
  await form.getByLabel("Environment and build").fill("macOS 15, demo build");
  await form
    .getByLabel("Reproduction, steps exercised, and results")
    .fill(
      "Dropped multiple files with spaces in their paths; all attached successfully.",
    );
  await form.getByRole("button", { name: "Record validation" }).click();
  await expect(page.locator(".drawer > .tag")).toHaveText("ready to merge");
  await page
    .getByLabel("Demo identity in detail", { exact: true })
    .selectOption("demo-author");
  await expect(page.locator('form[data-action="review"]')).toHaveCount(0);
  await expect(page.locator('form[data-action="validate"]')).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Claim task" }).first(),
  ).toBeDisabled();
});
test("roadmap and mobile detail fit the viewport", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await page.getByRole("button", { name: "Bugs & roadmap" }).click();
  await expect(
    page.getByRole("heading", { name: "Bugs & roadmap", exact: true }),
  ).toBeVisible();
  await page.locator(".roadmap-card").first().click();
  await expect(page.getByRole("dialog")).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({ path: testInfo.outputPath("mobile-detail.png") });
});
