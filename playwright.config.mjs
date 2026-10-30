import { defineConfig } from "@playwright/test";

const baseURL = "http://127.0.0.1:4174/mtgoxml/";
const executablePath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;

export default defineConfig({
  testDir: "./tests/browser",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  workers: 2,
  reporter: "list",
  use: {
    baseURL,
    browserName: "chromium",
    headless: true,
    trace: "retain-on-failure",
    launchOptions: executablePath ? { executablePath } : {},
  },
  webServer: {
    command: "node scripts/serve.mjs web 4174 /mtgoxml/",
    url: baseURL,
    reuseExistingServer: false,
    timeout: 10_000,
  },
});
