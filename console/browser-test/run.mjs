// The console's browser test: signs in, checks each account's figures against the API, stops and starts a
// strategy, shows a refusal in place, flattens an account, and checks phone width and the keyboard.
// Playwright is not a dependency of this repository: run it from a scratch directory that has the exact
// version installed, with PLAYWRIGHT_DIR pointing there. Settings come from the environment:
//   CONSOLE_URL        the console's address, such as https://127.0.0.1:8443
//   CONSOLE_PASSWORD   the login password
//   PLAYWRIGHT_DIR     the scratch directory holding node_modules/playwright
//   REFUSING_ACCOUNT   (optional) the address of an account whose strategies refuse to start
//   EXPECT_ACCOUNTS    (optional) how many accounts the overview must list, accounts that failed to start included
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

/** Basis points as a percentage with two places. */
function percent(bps) {
  const whole = String(Math.trunc(Math.abs(bps) / 100)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${bps < 0 ? "-" : ""}${whole}.${String(Math.abs(bps) % 100).padStart(2, "0")}%`;
}

/** MONKEY base units (18 decimals) as whole MONKEY with two places, truncated. */
function monkey(units) {
  const padded = units.padStart(19, "0");
  return usdc(`${padded.slice(0, -18).replace(/^0+(?=\d)/, "")}.${padded.slice(-18)}`);
}

/** The first six and last four characters of an address, as the console shortens it. */
function short(address) {
  return `${address.slice(0, 6)}…${address.slice(-4)}`;
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
  await page.getByText("Wrong password. Try again.").waitFor();
  await page.getByLabel("Password").fill(password);
  await page.keyboard.press("Enter");
  await page.getByRole("heading", { level: 1, name: "Accounts" }).waitFor();
  await page.getByText("Live", { exact: true }).waitFor();

  // Overview: each account's row matches the API.
  const listed = (await api("/api/accounts")).accounts;
  if (process.env.EXPECT_ACCOUNTS) {
    check(listed.length === Number(process.env.EXPECT_ACCOUNTS), `expected ${process.env.EXPECT_ACCOUNTS} accounts, API lists ${listed.length}`);
  }
  for (const account of listed.filter((a) => a.figures !== null)) {
    await eventually(`${short(account.address)}'s row matches the API`, async () => {
      const fresh = (await api("/api/accounts")).accounts.find((a) => a.address === account.address);
      const f = fresh.figures;
      const cells = await page.getByRole("row").filter({ hasText: short(account.address) }).getByRole("cell").allTextContents();
      const want = [
        fresh.state, usdc(f.equity), signed(f.realised_pnl), signed(f.unrealised_pnl),
        f.margin_level_bps === null ? "—" : percent(f.margin_level_bps), String(f.positions.length), monkey(f.monkey.newest_entitled),
      ];
      // The state cell may carry the day stop's note after the state.
      const matches = want.every((text, index) => (index === 0 ? cells[0]?.startsWith(text) : cells[index] === text));
      return matches ? true : { cells, want };
    });
  }

  // An account with no address is listed by its label, with its reason and no link, and the totals count it.
  for (const failed of listed.filter((a) => a.address === null)) {
    const row = page.getByRole("row").filter({ hasText: failed.label });
    await row.getByText(failed.error).waitFor();
    check((await row.getByRole("link").count()) === 0, "an account with no address has a link");
  }
  const { totals } = await api("/api/accounts");
  await page.getByText(`${totals.reporting} of ${totals.accounts} accounts reporting.`).waitFor();

  // Phone width: nothing scrolls sideways at 320 CSS px.
  await page.setViewportSize({ width: 320, height: 700 });
  check(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), "page scrolls sideways at 320px");
  await page.setViewportSize({ width: 1000, height: 800 });

  // Account page: stop and start a strategy, then see the API agree.
  const first = listed[0];
  const running = async () => (await api(`/api/accounts/${first.address}`)).strategies[0].running;
  await page.getByRole("link", { name: short(first.address) }).click();
  await page.getByRole("heading", { level: 1, name: short(first.address) }).waitFor();
  const card = page.getByRole("region", { name: /on /i }).first();
  await card.getByRole("button", { name: "Stop strategy" }).click();
  await card.getByRole("button", { name: "Start strategy" }).waitFor();
  check(!(await running()), "API still shows the strategy running after a stop");
  await card.getByRole("button", { name: "Start strategy" }).click();
  await card.getByRole("button", { name: "Stop strategy" }).waitFor();
  check(await running(), "API shows the strategy stopped after a start");

  // A refusal shows at the control that caused it.
  if (process.env.REFUSING_ACCOUNT) {
    await page.goto(`${url}/accounts/${process.env.REFUSING_ACCOUNT}`);
    const refusing = page.getByRole("region", { name: /on /i }).first();
    await refusing.getByRole("button", { name: "Start strategy" }).click();
    await refusing.getByText(/^Failed: /).waitFor();
    check((await page.getByText(/^Failed: /).count()) === 1, "a failure showed somewhere other than its control");
    // The daily loss stop the API reports shows on the page, and the controls stay.
    const stopped = (await api(`/api/accounts/${process.env.REFUSING_ACCOUNT}`)).day_stop;
    if (stopped !== null) {
      await page.getByText(/^Stopped for the day: daily loss .* USD, trading resumes at /).waitFor();
      check(await page.getByRole("button", { name: /^(Stop|Start) account$/ }).isEnabled(), "the account's control is not offered during a day stop");
    }
  }

  // Flatten asks twice, then the positions are gone in the page and in the API.
  await page.goto(`${url}/accounts/${first.address}`);
  await page.getByRole("heading", { level: 1, name: short(first.address) }).waitFor();
  await page.getByRole("button", { name: "Flatten account" }).click();
  await page.getByRole("button", { name: "Confirm flatten" }).click();
  await page.getByText("No open positions.").waitFor();
  await eventually("the API shows no positions", async () => (await api(`/api/accounts/${first.address}`)).figures.positions.length === 0);

  check(foreign.length === 0, `requests left the console's origin: ${foreign.join(", ")}`);
  check(problems.length === 0, `page errors: ${problems.join("; ")}`);
  process.stdout.write("browser test passed\n");
} finally {
  await browser.close();
}
