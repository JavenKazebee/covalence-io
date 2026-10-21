// M2 acceptance: every "Done when" item in docs/milestones/m2-first-ui.md,
// through a browser, against the real server (see playwright.config.ts).

import { expect, test, type Page } from "@playwright/test";

test.describe.configure({ mode: "serial" });

/** The value cell of the State row for `path`. */
const stateValue = (page: Page, path: string) => page.locator(`tr[data-path="${path}"] td`).nth(1);
const stateQuality = (page: Page, path: string) => page.locator(`tr[data-path="${path}"] td`).nth(2);

test("the server serves the UI at /, next to the API", async ({ page, request }) => {
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Covalence IO" })).toBeVisible();
  await expect(page.getByRole("status").filter({ hasText: "Plugin host" })).toHaveText(/Plugin host ready/, {
    timeout: 15_000,
  });
  // The API is still where it was.
  expect((await request.get("/health")).ok()).toBe(true);
  // A deep link that isn't a file gets the app (SPA fallback).
  await page.goto("/some/deep/link");
  await expect(page.getByRole("heading", { name: "Covalence IO" })).toBeVisible();
});

test("a Videohub connection is created from the manifest's config form", async ({ page }) => {
  await page.goto("/#/connections");
  const create = page.getByRole("button", { name: "Create connection" });
  await expect(create).toBeVisible({ timeout: 15_000 });

  // Required fields are checked before anything is sent.
  await create.click();
  await expect(page.getByText("Required")).toBeVisible();

  await page.getByLabel("Alias").fill("vh");
  await page.getByLabel("IP address").fill("127.0.0.1");
  await expect(page.getByLabel("Port")).toHaveValue("9990");
  await create.click();

  await expect(page.getByRole("status").filter({ hasText: "Created" })).toHaveText("Created vh.");
  const row = page.getByRole("row").filter({ has: page.getByRole("link", { name: "vh", exact: true }) });
  await expect(row).toContainText("simulate");
  await expect(row).toContainText('"port":9990');
});

test("State shows vh.outputs[*].source updating live as routes change", async ({ page, request }) => {
  await page.goto("/#/state/vh.outputs");
  const source = stateValue(page, "vh.outputs[2].source");
  await expect(source).toHaveText(/^\d+$/);

  // Route from outside the page: it must show up without a reload.
  for (const input of [9, 4]) {
    const res = await request.post("/actions", { data: { action: "vh.route", params: { output: 2, input } } });
    expect((await res.json()).ok).toBe(true);
    await expect(source).toHaveText(String(input));
  }
  await expect(stateQuality(page, "vh.outputs[2].source")).toHaveText("good");
});

test("State shows stale while the plugin host is down", async ({ page, request }) => {
  await page.goto("/#/state/vh.outputs");
  await expect(stateQuality(page, "vh.outputs[0].source")).toHaveText("good");

  // The host restarts within its budget, so record what the page shows while
  // it's down rather than hoping to catch it with a poll.
  await page.evaluate(() => {
    const seen = { stale: false, down: false };
    (window as unknown as { seen: typeof seen }).seen = seen;
    const check = () => {
      if (document.querySelector('tr[data-path="vh.outputs[0].source"] .badge.stale')) seen.stale = true;
      if (document.querySelector(".host-status")?.textContent?.includes("Plugin host down")) seen.down = true;
    };
    new MutationObserver(check).observe(document.body, { subtree: true, childList: true, characterData: true });
  });

  const { host } = await (await request.get("/health")).json();
  process.kill(host.pid, "SIGKILL");

  await expect
    .poll(() => page.evaluate(() => (window as unknown as { seen: object }).seen), { timeout: 10_000 })
    .toEqual({ stale: true, down: true });
  // And back once the host has restarted and the connection is recreated.
  await expect(page.getByRole("status").filter({ hasText: "Plugin host" })).toHaveText(/Plugin host ready/, {
    timeout: 15_000,
  });
  await expect(stateQuality(page, "vh.outputs[0].source")).toHaveText("good", { timeout: 15_000 });
});

test("the manifest's action form runs vh.route, and the result is in the journal", async ({ page }) => {
  await page.goto("/#/actions");
  await expect(page.getByLabel("Connection")).toHaveValue(/.+/);
  await page.getByLabel("Connection").selectOption({ label: "vh" });
  await expect(page.getByLabel("Action")).toHaveValue("route");

  // The dropdowns are filled from vh.inputs and vh.outputs.
  const output = page.getByLabel("Output");
  await expect(output.locator("option", { hasText: "3: Output 4" })).toHaveCount(1);
  await output.selectOption({ label: "3: Output 4" });
  await page.getByLabel("Input").selectOption({ label: "7: Input 8" });
  await page.getByRole("button", { name: "Run vh.route" }).click();

  const result = page.locator(".action-run").first();
  await expect(result).toContainText("OK");
  await expect(result).toContainText('{"output":3,"input":7}');

  await result.getByRole("link").click();
  await expect(page).toHaveURL(/#\/journal\/\d+$/);
  const entry = page.locator('tr[data-kind="action.result"]').first();
  await expect(entry).toContainText("vh.route OK");
});

test("clicking a journal entry shows its why chain", async ({ page }) => {
  await page.goto("/#/journal");
  const patch = page.locator('tr[data-kind="state.patch"]', { hasText: "vh.outputs[3].source = 7" }).first();
  await patch.click();

  const drawer = page.getByRole("complementary", { name: "Why" });
  await expect(drawer).toBeVisible();
  const chain = drawer.locator("li[data-seq]");
  await expect(chain).toHaveCount(2);
  await expect(chain.nth(0)).toContainText("state.patch");
  await expect(chain.nth(1)).toContainText("action.requested");
  await expect(chain.nth(1)).toContainText('vh.route {"input":7,"output":3}');
});
