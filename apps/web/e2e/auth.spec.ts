import { expect, test } from "@playwright/test";

const adminPassword = process.env.SCION_E2E_PASSWORD ?? "correct-horse-1";
const nextPassword = "correct-horse-2";

test("login, password change, and repository visibility", async ({ page }) => {
  await page.goto("/login");
  await page.getByLabel("Username").fill("ada");
  await page.getByLabel("Password").fill(adminPassword);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("heading", { name: "Change password" })).toBeVisible();
  await page.getByLabel("Current password").fill(adminPassword);
  await page.getByLabel("New password").fill(nextPassword);
  await page.getByRole("button", { name: "Update password" }).click();
  await expect(page.getByRole("heading", { name: "Scion" })).toBeVisible();

  await page.getByLabel("Username").fill("ada");
  await page.getByLabel("Password").fill(nextPassword);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("heading", { name: "Repositories" })).toBeVisible();

  await page.getByRole("link", { name: "New repository" }).click();
  await page.getByLabel("Name").fill("widgets");
  await page.getByLabel("Description").fill("demo");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("heading", { name: "ada/widgets" })).toBeVisible();

  await page.getByRole("link", { name: "Users" }).click();
  await page.getByPlaceholder("username").fill("bea");
  await page.getByPlaceholder("password").fill("bea-password-1");
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("cell", { name: "bea" })).toBeVisible();

  await page.context().clearCookies();
  await page.goto("/login");
  await page.getByLabel("Username").fill("bea");
  await page.getByLabel("Password").fill("bea-password-1");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("No repositories yet.")).toBeVisible();

  await page.context().clearCookies();
  await page.goto("/login");
  await page.getByLabel("Username").fill("ada");
  await page.getByLabel("Password").fill(nextPassword);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("heading", { name: "Repositories" })).toBeVisible();
  await page.goto("/r/ada/widgets/settings/members");
  await page.getByPlaceholder("username").fill("bea");
  await page.getByRole("button", { name: "Grant" }).click();
  await expect(page.getByRole("cell", { name: "bea" })).toBeVisible();

  await page.context().clearCookies();
  await page.goto("/login");
  await page.getByLabel("Username").fill("bea");
  await page.getByLabel("Password").fill("bea-password-1");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("link", { name: "ada/widgets" })).toBeVisible();
  await page.getByRole("link", { name: "ada/widgets" }).click();
  await expect(page.getByText("role read")).toBeVisible();
  await expect(page.getByRole("button", { name: "Land edit" })).toHaveCount(0);
});
