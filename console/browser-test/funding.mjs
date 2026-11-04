// The console's funding browser test: signs in, declares for two accounts after reading the statement, deposits,
// claims and withdraws for both through the page, sees a refused deposit fail at its control, and checks every
// figure the page shows against the API. Run it as run.mjs is run (see that file for Playwright), with:
//   CONSOLE_URL, CONSOLE_PASSWORD, PLAYWRIGHT_DIR   as for run.mjs
//   FUNDING_ACCOUNTS   (optional) two account names, comma separated; default the first two the API lists
//   DEPOSIT_USDC       (optional) the amount to deposit for each, default 100
//   WITHDRAW_USDC      (optional) the amount to withdraw from each, default 40
//   REFUSED_DEPOSIT    (optional) an amount the deposit must fail on, default 1
import { createRequire } from "node:module";
import process from "node:process";

const url = process.env.CONSOLE_URL;
const password = process.env.CONSOLE_PASSWORD;
const playwrightDir = process.env.PLAYWRIGHT_DIR;
if (!url || !password || !playwrightDir) {
  throw new Error("set CONSOLE_URL, CONSOLE_PASSWORD and PLAYWRIGHT_DIR");
}
const deposit = process.env.DEPOSIT_USDC ?? "100";
const withdraw = process.env.WITHDRAW_USDC ?? "40";
const refused = process.env.REFUSED_DEPOSIT ?? "1";
const { chromium } = createRequire(`${playwrightDir}/`)("playwright");

const SCALE = 10n ** 18n;

/** A decimal string as units of 10^-18. */
function units(value) {
  const [whole, fraction = ""] = value.split(".");
  return BigInt(whole) * SCALE + BigInt(fraction.padEnd(18, "0"));
}

/** What the page shows for a decimal: grouped thousands, trailing zeros dropped. */
function shown(value) {
  const [whole, fraction = ""] = value.split(".");
  const trimmed = fraction.replace(/0+$/, "");
  return `${whole.replace(/\B(?=(\d{3})+(?!\d))/g, ",")}${trimmed === "" ? "" : `.${trimmed}`}`;
}

function check(condition, message) {
  if (!condition) {
    throw new Error(`funding browser test failed: ${message}`);
  }
}

async function eventually(description, read) {
  const deadline = Date.now() + 60_000;
  let last;
  while (Date.now() < deadline) {
    last = await read();
    if (last === true) {
      return;
    }
    await new Promise((resolve) => { setTimeout(resolve, 250); });
  }
  throw new Error(`funding browser test failed: ${description} (last: ${JSON.stringify(last)})`);
}

const browser = await chromium.launch();
const context = await browser.newContext({ ignoreHTTPSErrors: true, viewport: { width: 1100, height: 900 } });
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
  if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
    problems.push(message.text());
  }
});
const funding = async () => (await context.request.get(`${url}/api/funding`)).json();
const row = async (name) => (await funding()).accounts.find((account) => account.name === name);

try {
  await page.goto(url);
  await page.getByLabel("Password").fill(password);
  await page.keyboard.press("Enter");
  await page.getByRole("heading", { level: 1, name: "Accounts" }).waitFor();
  const listed = (await funding()).accounts.map((account) => account.name);
  const names = (process.env.FUNDING_ACCOUNTS ?? listed.slice(0, 2).join(",")).split(",");
  check(names.length === 2 && names.every((name) => listed.includes(name)), `two listed accounts needed, got ${names}`);

  await page.getByRole("navigation", { name: "Pages" }).getByRole("link", { name: "Funding" }).click();
  await page.getByRole("heading", { level: 1, name: "Funding" }).waitFor();
  await page.getByText("Every action here pays only the account's own address").waitFor();
  for (const name of names) {
    await page.getByRole("checkbox", { name: `Select ${name}` }).check();
  }

  // Declare: nothing is sent until the statement is chosen and confirmed.
  const declaration = page.getByRole("region", { name: "Declaration" });
  const declareButton = declaration.getByRole("button", { name: /^Declare for/ });
  if ((await Promise.all(names.map(async (name) => (await row(name)).declared === null))).some(Boolean)) {
    check(await declareButton.isDisabled(), "declare is enabled before a statement is read and confirmed");
    await declaration.getByRole("radio", { name: "I attest that I am a professional client." }).check();
    check(await declareButton.isDisabled(), "declare is enabled before the confirmation");
    await declaration.getByRole("checkbox", { name: /I have read the statement above/ }).check();
    await declareButton.click();
    for (const name of names) {
      await eventually(`${name} declared`, async () => (await row(name)).declared === "professional");
    }
  }

  // Deposit, claim, withdraw for both; each figure the page shows matches the API after the transaction lands.
  const before = Object.fromEntries(await Promise.all(names.map(async (name) => [name, await row(name)])));
  const depositSection = page.getByRole("region", { name: "Deposit USDC" });
  await depositSection.getByLabel("Amount per account (USDC)").fill(deposit);
  await depositSection.getByRole("button", { name: "Deposit for 2 accounts" }).click();
  for (const name of names) {
    const was = before[name];
    await eventually(`${name}'s deposit landed`, async () => {
      const now = await row(name);
      return units(now.walletUsdc) === units(was.walletUsdc) - units(deposit) && units(now.balance ?? "0") >= units(was.balance ?? "0") + units(deposit);
    });
  }
  await depositSection.getByText("Landed", { exact: false }).first().waitFor();

  const claimSection = page.getByRole("region", { name: "Claim MONKEY" });
  const claimable = (await Promise.all(names.map(async (name) => [name, await row(name)]))).filter(([, a]) => units(a.unclaimedMonkey) > 0n);
  check(claimable.length > 0, "no account has MONKEY to claim");
  await claimSection.getByRole("button", { name: /^Claim for/ }).click();
  for (const [name] of claimable) {
    await eventually(`${name} claimed`, async () => units((await row(name)).unclaimedMonkey) === 0n);
  }

  const mid = Object.fromEntries(await Promise.all(names.map(async (name) => [name, await row(name)])));
  const withdrawSection = page.getByRole("region", { name: "Withdraw USDC" });
  await withdrawSection.getByLabel("Amount per account (USDC)").fill(withdraw);
  await withdrawSection.getByRole("button", { name: "Withdraw for 2 accounts" }).click();
  for (const name of names) {
    await eventually(`${name}'s withdrawal paid`, async () => {
      const now = await row(name);
      return units(now.walletUsdc) === units(mid[name].walletUsdc) + units(withdraw) && now.queued.length === 0 && now.unpaid.length === 0;
    });
  }

  // The page shows what the API says, exactly.
  for (const name of names) {
    const account = await row(name);
    await eventually(`${name}'s row shows the API's figures`, async () => {
      const cells = await page.getByRole("row").filter({ hasText: name }).getByRole("cell").allTextContents();
      const want = ["", "professional client", shown(account.walletUsdc), shown(account.walletEth), shown(account.balance), shown(account.unclaimedMonkey)];
      return want.every((text, index) => cells[index] === text) ? true : { cells, want };
    });
  }

  // A deposit the vault refuses fails at its control, with the reason, and nowhere else.
  await page.getByRole("checkbox", { name: `Select ${names[1]}` }).uncheck();
  await depositSection.getByLabel("Amount per account (USDC)").fill(refused);
  await depositSection.getByRole("button", { name: "Deposit for 1 account" }).click();
  await depositSection.getByText(/Failed: ./).waitFor();
  check((await page.getByText(/Failed: /).count()) === 1, "a failure showed somewhere other than its control");

  check(foreign.length === 0, `requests left the console's origin: ${foreign.join(", ")}`);
  check(problems.length === 0, `page errors: ${problems.join("; ")}`);
  process.stdout.write("funding browser test passed\n");
} finally {
  await browser.close();
}
