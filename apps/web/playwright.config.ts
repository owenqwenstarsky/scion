import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  use: {
    baseURL: process.env.SCION_WEB_ORIGIN ?? "http://localhost:3000",
  },
});
