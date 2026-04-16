import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'child_process'
import path from 'path'
import { fileURLToPath } from 'url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

// Resolve the pre-built native binary (built with `cargo build`).
const NATIVE_BIN = path.resolve(
  __dirname,
  '../../../../target/debug/ref-roundtrip-native',
)

let native: ChildProcess | null = null
let peerKey = ''

test.beforeAll(async () => {
  native = spawn(NATIVE_BIN, [], {
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  // Surface native stderr to the test console for debugging.
  native.stderr?.on('data', (d: Buffer) => {
    process.stderr.write(`[native] ${String(d)}`)
  })

  peerKey = await new Promise<string>((resolve, reject) => {
    native!.stdout!.on('data', (data: Buffer) => {
      const text = data.toString()
      const match = text.match(/^PEER_KEY:(\S+)/m)
      if (match) resolve(match[1])
    })
    native!.on('error', reject)
    native!.on('exit', (code) => {
      if (code !== null && peerKey === '') {
        reject(new Error(`native exited early with code ${code}`))
      }
    })
    // Wait up to 20 s for the native peer to bind and print its key.
    setTimeout(() => reject(new Error('timeout waiting for PEER_KEY')), 20_000)
  })
})

test.afterAll(() => {
  native?.kill('SIGTERM')
})

test('all ref round-trip scenarios pass', async ({ page }) => {
  // Route console messages to the test runner output.
  page.on('console', (msg) => console.log(`[browser] ${msg.type()}: ${msg.text()}`))
  page.on('pageerror', (err) => console.error(`[browser] page-error: ${err}`))
  page.on('requestfailed', (req) =>
    console.log(`[browser] 404/fail: ${req.url()} — ${req.failure()?.errorText}`),
  )

  await page.goto(`/?peer=${peerKey}`)

  // Periodically log the status text so we can see where the hang is.
  const statusPoll = setInterval(async () => {
    const text = await page.locator('#status').textContent().catch(() => '')
    const anyResults = await page.locator('.test-case').count().catch(() => -1)
    console.log(`[poll] status="${text}" results=${anyResults}`)
    const errors = await page.locator('.fatal').allTextContents().catch(() => [])
    if (errors.length) console.log(`[poll] fatal=${errors.join('; ')}`)
  }, 5_000)

  // Wait for all test cases to settle — timeout is generous to cover
  // the 15s lookupDepot + per-test timeouts.
  const resultDiv = page.locator('#results[data-done="true"]')
  await resultDiv.waitFor({ timeout: 90_000 }).finally(() => clearInterval(statusPoll))

  // Check for fatal init errors first.
  const fatalErrors = await page.locator('.fatal').allTextContents()
  if (fatalErrors.length > 0) {
    throw new Error(`Fatal init error: ${fatalErrors.join('; ')}`)
  }

  // Each test case row must have class 'pass'.
  const testCases = page.locator('.test-case')
  const count = await testCases.count()
  expect(count).toBeGreaterThan(0)

  const failed: string[] = []
  for (let i = 0; i < count; i++) {
    const el = testCases.nth(i)
    const isPassed = await el.evaluate((node) => node.classList.contains('pass'))
    if (!isPassed) {
      const text = await el.textContent()
      failed.push(text ?? `test-case[${i}]`)
    }
  }

  if (failed.length > 0) {
    throw new Error(`Failed test cases:\n${failed.join('\n')}`)
  }
})
