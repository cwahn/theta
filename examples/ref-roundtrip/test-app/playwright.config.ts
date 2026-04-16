import { defineConfig, devices } from '@playwright/test'

export default defineConfig({
  testDir: './tests',
  timeout: 120_000,
  expect: { timeout: 90_000 },

  webServer: {
    // vite-plugin-theta will run wasm-pack and serve the app.
    command: 'npx vite --port 5174',
    port: 5174,
    timeout: 120_000,
    reuseExistingServer: false,
  },

  use: {
    baseURL: 'http://localhost:5174',
    // Show browser console in test output.
    launchOptions: { args: ['--enable-logging'] },
  },

  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
})
