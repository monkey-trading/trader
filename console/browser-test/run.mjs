// The console's browser test: signs in, checks each account's figures against the API, stops and starts a
// strategy, shows a refusal in place, flattens an account, and checks phone width and the keyboard.
// Playwright is not a dependency of this repository: run it from a scratch directory that has the exact
// version installed, with PLAYWRIGHT_DIR pointing there. Settings come from the environment:
//   CONSOLE_URL        the console's address, such as https://127.0.0.1:8443
//   CONSOLE_PASSWORD   the login password
//   PLAYWRIGHT_DIR     the scratch directory holding node_modules/playwright
//   REFUSING_ACCOUNT   (optional) the name of an account whose strategies refuse to start
//   EXPECT_ACCOUNTS    (optional) how many accounts the overview must list
import { createRequire } from "node:module";
import process from "node:process";

const url = process.env.CONSOLE_URL;
const password = process.env.CONSOLE_PASSWORD;
const playwrightDir = process.env.PLAYWRIGHT_DIR;
if (!url || !password || !playwrightDir) {
  throw new Error("set CONSOLE_URL, CONSOLE_PASSWORD and PLAYWRIGHT_DIR");
}
const { chromium } = createRequire(`${playwrightDir}/`)("playwright");

/** Truncates a decimal string to two places with thousands separators, as the console shows USDC. */
function usdc(value) {
  const negative = value.startsWith("-");
  const [whole, fraction = ""] = (negative ? value.slice(1) : value).split(".");
  const kept = fraction.padEnd(2, "0").slice(0, 2);
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const text = `${grouped}.${kept}`;
  return negative && /[1-9]/.test(text) ? `-${text}` : text;
}

/** The same with a plus sign for a gain. */
function signed(value) {
  const text = usdc(value);
  return /[1-9]/.test(text) && !text.startsWith("-") ? `+${text}` : text;
}

function check(condition, message) {
  if (!condition) {
    throw new Error(`browser test failed: ${message}`);
  }
}

async function eventually(description, read) {
  const deadline = Date.now() + 15_000;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (last === true) {
      return;
    }
    await new Promise((resolve) => { setTimeout(resolve, 200); });
  }
  throw new Error(`browser test failed: ${description} (last: ${JSON.stringify(last)})`);
}

const browser = await chromium.launch();
const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1000, height: 800 } });
const page = await context.newPage();
const foreign = [];
const problems = [];
page.on("request", (request) => {
  if (new URL(request.url()).origin !== new URL(url).origin && !request.url().startsWith("data:")) {
    foreign.push(request.url());
  }
});
page.on("pageerror", (error) => problems.push(error.message));
page.on("console", (message) => {
  // The browser logs every 401 and refusal the test causes on purpose; script errors are what matter.
  if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
    problems.push(message.text());
  }
});
const api = async (path) => (await context.request.get(`${url}${path}`)).json();

try {
  // Login: the skip link comes first in the tab order, a wrong password is refused at the field.
  await page.goto(url);
  await page.getByRole("heading", { level: 1, name: "Sign in" }).waitFor();
  await page.keyboard.press("Tab");
  check(await page.getByRole("link", { name: "Skip to content" }).evaluate((el) => el === document.activeElement), "skip link not first");
  await page.getByLabel("Password").fill("not-the-password");
  await page.keyboard.press("Enter");
  await page.getByText("The password was not accepted.").waitFor();
  await page.getByLabel("Password").fill(password);
  await page.keyboard.press("Enter");
  await page.getByRole("heading", { level: 1, name: "Accounts" }).waitFor();
  await page.getByText("Live", { exact: true }).waitFor();

  // Overview: each account's row matches the API.
  const listed = (await api("/api/accounts")).accounts;
  if (process.env.EXPECT_ACCOUNTS) {
    check(listed.length === Number(process.env.EXPECT_ACCOUNTS), `expected ${process.env.EXPECT_ACCOUNTS} accounts, API lists ${listed.length}`);
  }
  for (const account of listed) {
    await eventually(`${account.name}'s row matches the API`, async () => {
      const fresh = (await api("/api/accounts")).accounts.find((a) => a.address === account.address);
      const cells = await page.getByRole("row").filter({ hasText: fresh.name }).getByRole("cell").allTextContents();
      const want = [fresh.state, usdc(fresh.equity), signed(fresh.realisedPnl), signed(fresh.unrealisedPnl)];
      return want.every((text, index) => cells[index] === text) ? true : { cells, want };
    });
  }

  // Phone width: nothing scrolls sideways at 320 CSS px.
  await page.setViewportSize({ width: 320, height: 700 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "page scrolls sideways at 320px");
  await page.setViewportSize({ width: 1000, height: 800 });

  // Account page: stop and start a strategy, then see the API agree.
  const first = listed[0];
  await page.getByRole("link", { name: first.name, exact: true }).click();
  await page.getByRole("heading", { level: 1, name: first.name }).waitFor();
  const card = page.getByRole("region", { name: /on /i }).first();
  await card.getByRole("button", { name: "Stop strategy" }).click();
  await card.getByRole("button", { name: "Start strategy" }).waitFor();
  const stopped = (await api(`/api/accounts/${first.address}`)).account.strategies[0];
  check(stopped.state === "stopped", "API still shows the strategy running after a stop");
  await card.getByRole("button", { name: "Start strategy" }).click();
  await card.getByRole("button", { name: "Stop strategy" }).waitFor();
  check((await api(`/api/accounts/${first.address}`)).account.strategies[0].state === "running", "API shows the strategy stopped after a start");

  // A refusal shows at the control that caused it.
  if (process.env.REFUSING_ACCOUNT) {
    await page.goto(`${url}/`);
    await page.getByRole("link", { name: process.env.REFUSING_ACCOUNT, exact: true }).click();
    const refusing = page.getByRole("region", { name: /on /i }).first();
    const stopButton = refusing.getByRole("button", { name: "Start strategy" });
    if ((await stopButton.count()) === 0) {
      await refusing.getByRole("button", { name: "Stop strategy" }).click();
      await stopButton.waitFor();
    }
    await stopButton.click();
    await refusing.getByText(/^Failed: /).waitFor();
    check((await page.getByText(/^Failed: /).count()) === 1, "a failure showed somewhere other than its control");
  }

  // Flatten asks twice, then the positions are gone in the page and in the API.
  await page.goto(`${url}/accounts/${first.address}`);
  await page.getByRole("heading", { level: 1, name: first.name }).waitFor();
  await page.getByRole("button", { name: "Flatten account" }).click();
  await page.getByRole("button", { name: "Confirm flatten" }).click();
  await page.getByText("No open positions.").waitFor();
  await eventually("the API shows no positions", async () => (await api(`/api/accounts/${first.address}`)).account.positions.length === 0);

  check(foreign.length === 0, `requests left the console's origin: ${foreign.join(", ")}`);
  check(problems.length === 0, `page errors: ${problems.join("; ")}`);
  process.stdout.write("browser test passed\n");
} finally {
  await browser.close();
}
