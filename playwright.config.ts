import { defineConfig } from '@playwright/test'
export default defineConfig({
  testDir: './tests/e2e',
  workers: 1,
  globalSetup: './tests/e2e/setup.ts',
  webServer: {
    command: 'npm run dev:desktop',
    url: 'http://localhost:1420',
    reuseExistingServer: true,
    timeout: 60_000,
  },
  timeout: 30_000,
  use: {
    baseURL: 'http://localhost:1420',
    browserName: 'chromium',
    channel: 'msedge',
    headless: true,
    actionTimeout: 5000,
    trace: 'off',
    screenshot: 'off',
    video: 'off',
  },
  reporter: 'list',
})
