import { test, expect } from "@playwright/test";
async function role(page, name) {
  await page.getByRole("button", { name: "Try a role" }).click();
  await page.getByRole("dialog").getByLabel(name, { exact: true }).check();
  await page.getByRole("button", { name: "Open role preview" }).click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
}
test("clear navigation, searching, original links and stars persist", async ({
  page,
}, testInfo) => {
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: /Good changes/ }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Pull requests", exact: true })
    .click();
  await expect(page.locator(".list .pr-card")).toHaveCount(6);
  await page.getByLabel("Search work").fill("Windows");
  await expect(page.locator(".list .pr-card")).toHaveCount(1);
  await page.locator(".list [data-star]").click();
  await page.getByRole("button", { name: /Starred/ }).click();
  await expect(page.locator(".list .pr-card")).toHaveCount(1);
  await page.reload();
  await page.getByRole("button", { name: /Starred/ }).click();
  await expect(page.locator(".list .pr-card")).toHaveCount(1);
  await page.evaluate(() => document.fonts.ready);
  expect(
    await page
      .locator(".mark")
      .evaluate((i) => i.complete && i.naturalWidth > 0),
  ).toBe(true);
  await page.screenshot({
    path: testInfo.outputPath("queue.png"),
    fullPage: true,
  });
  expect(errors).toEqual([]);
});
test("a guest report asks for a name and saves it in the browser", async ({
  page,
}) => {
  await page.goto("/");
  await page
    .getByRole("button", { name: "Report a bug", exact: true })
    .first()
    .click();
  const form = page.getByRole("dialog");
  await form.getByLabel("Your display name").fill("Browser guest");
  await form.getByLabel("What went wrong?").fill("Guest attachment issue");
  await form
    .getByLabel("Steps to reproduce, expected and actual behavior")
    .fill("Dragging a file should attach it, but nothing appears.");
  await form.getByRole("button", { name: "Submit report" }).click();
  await expect(
    page.getByRole("heading", { name: "Guest attachment issue", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("unverified guest", { exact: true }),
  ).toBeVisible();
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Guest attachment issue", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Propose a feature", exact: true })
    .first()
    .click();
  await expect(
    page.getByRole("dialog").getByLabel("Your display name"),
  ).toHaveValue("Browser guest");
  await page.keyboard.press("Escape");
});
test("reviewer claims a task and records a revision-specific review in a dialog", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90002");
  await role(page, "Reviewer");
  await page
    .locator(".task")
    .filter({ hasText: "code review" })
    .getByRole("button", { name: "Claim task" })
    .click();
  await expect(
    page.locator(".task").filter({ hasText: "code review" }),
  ).toContainText("demo-reviewer");
  await page
    .getByRole("button", { name: "Submit code review", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Approve", { exact: true }).check();
  await dialog
    .getByLabel("Findings and scope reviewed")
    .fill("Checked dropped path normalization and regression coverage.");
  await dialog.getByRole("button", { name: "Submit review" }).click();
  await expect(page.locator(".detail-title")).toContainText("needs validation");
  await page.reload();
  await expect(page.locator(".evidence")).toContainText([
    "Checked dropped path normalization",
  ]);
});
test("validator finishes the packet and an author cannot self-verify", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90004");
  await role(page, "Validator");
  await page
    .getByRole("button", { name: "Record validation", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Passed", { exact: true }).check();
  await dialog.getByLabel("Environment and build").fill("macOS 15, test build");
  await dialog
    .getByLabel("Reproduction, steps exercised, and results")
    .fill("Dropped multiple files with spaces; all attached successfully.");
  await dialog
    .getByRole("button", { name: "Record validation", exact: true })
    .click();
  await expect(page.locator(".detail-title")).toContainText("ready to merge");
  await role(page, "Contributor");
  await expect(
    page.getByRole("button", { name: "Code review", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Behavior validation", exact: true }),
  ).toBeDisabled();
});
test("maintainer hotfix path accepts scope without bypassing remaining evidence", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90001");
  await role(page, "Maintainer");
  await page.getByRole("button", { name: "Hotfix path" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Area", { exact: true }).fill("browser");
  await dialog
    .getByLabel("Hotfix scope and reason")
    .fill("Urgent focused fix for the annotation regression.");
  await dialog.getByRole("button", { name: "Accept hotfix scope" }).click();
  await expect(page.locator(".facts")).toContainText("urgent priority");
  await expect(page.locator(".panel").first()).toContainText(
    "Urgent focused fix",
  );
  await expect(page.locator(".blockers")).toContainText(
    "Independent code review needed",
  );
  await expect(page.locator(".detail-title")).not.toContainText(
    "ready to merge",
  );
});
test("board dragging opens required action dialog rather than assigning readiness", async ({
  page,
}) => {
  await page.goto("/");
  await role(page, "Maintainer");
  await page
    .getByRole("button", { name: "Pull requests", exact: true })
    .click();
  await page.getByRole("button", { name: "Board", exact: true }).click();
  const card = page.locator(".board-card").first();
  await card.dragTo(page.locator('[data-drop-stage="needs-direction"]'));
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Decide direction", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
});
test("mobile detail and input dialog fit the viewport", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/?item=pr%3A90005");
  await expect(page.getByRole("heading", { name: /Windows/ })).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page
    .getByRole("button", { name: "Report a bug", exact: true })
    .count()
    .then(async (count) => {
      if (count)
        await page
          .getByRole("button", { name: "Report a bug", exact: true })
          .first()
          .click();
      else {
        await page.getByRole("button", { name: "Bugs", exact: true }).click();
        await page
          .getByRole("button", { name: "Report a bug", exact: true })
          .click();
      }
    });
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.screenshot({
    path: testInfo.outputPath("mobile-dialog.png"),
    fullPage: true,
  });
  expect(
    await page
      .getByRole("dialog")
      .evaluate((d) => d.getBoundingClientRect().width <= innerWidth),
  ).toBe(true);
});
test("a contributor can share demand without changing readiness", async ({
  page,
}) => {
  await page.goto("/?item=pr%3A90005");
  await expect(
    page.getByRole("button", { name: "Share a use case" }),
  ).toBeDisabled();
  await role(page, "Contributor");
  await page.getByRole("button", { name: "Share a use case" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("I need this", { exact: true }).check();
  await dialog
    .getByLabel("Your use case or reason")
    .fill("This fix unblocks our Windows installation.");
  await dialog.getByRole("button", { name: "Share signal" }).click();
  await expect(page.locator(".support")).toContainText("1 demand signals");
  await expect(page.locator(".detail-title")).toContainText("ready to merge");
});
