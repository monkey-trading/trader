// The console's funding browser test: signs in, opens the Funding page, and, for two accounts named by
// FUNDING_ACCOUNTS, declares them, deposits into them (one from its own wallet, one from the funding wallet),
// claims their MONKEY and withdraws from them, each through the page, checking each job's state in the page and
// the API. Phases: `prepare` declares and deposits, `settle` claims and withdraws, `all` runs both. Settings
// come from the environment, as for run.mjs:
//   CONSOLE_URL, CONSOLE_PASSWORD, PLAYWRIGHT_DIR   as there
//   FUNDING_ACCOUNTS   the two addresses, lower case, comma separated
//   FUNDING_PHASE      prepare, settle or all (default all)
//   DEPOSIT_USDC       what each deposit is (default 1000)
//   WITHDRAW_USDC      what each withdrawal is (default 100)
import { createRequire } from "node:module";
import process from "node:process";

const url = process.env.CONSOLE_URL;
const password = process.env.CONSOLE_PASSWORD;
const playwrightDir = process.env.PLAYWRIGHT_DIR;
const accounts = (process.env.FUNDING_ACCOUNTS ?? "").split(",").filter((a) => a !== "");
const phase = process.env.FUNDING_PHASE ?? "all";
const deposit = process.env.DEPOSIT_USDC ?? "1000";
const withdrawal = process.env.WITHDRAW_USDC ?? "100";
if (!url || !password || !playwrightDir || accounts.length !== 2) {
  throw new Error("set CONSOLE_URL, CONSOLE_PASSWORD, PLAYWRIGHT_DIR and two FUNDING_ACCOUNTS");
}
const { chromium } = createRequire(`${playwrightDir}/`)("playwright");

function check(condition, message) {
  if (!condition) {
    throw new Error(`funding browser test failed: ${message}`);
  }
}

async function eventually(description, read) {
  const deadline = Date.now() + 300_000;
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
const problems = [];
page.on("pageerror", (error) => problems.push(error.message));
page.on("console", (message) => {
  if (message.type() === "error" && !message.text().startsWith("Failed to load resource")) {
    problems.push(message.text());
  }
});
const funding = async () => (await context.request.get(`${url}/api/funding`)).json();
/** `address`'s figures, once the bot has read them. */
async function figures(address) {
  let account;
  await eventually(`${address}'s figures are read`, async () => {
    account = (await funding()).accounts.find((a) => a.address === address);
    return (account !== undefined && !account.reading && account.error === null) || account;
  });
  return account;
}

/** Selects exactly `wanted` in the table. */
async function choose(wanted) {
  for (const address of accounts) {
    const box = page.getByRole("checkbox", { name: `Select ${address}` });
    if ((await box.isChecked()) !== wanted.includes(address)) {
      await box.click();
    }
  }
}

/** Waits until the API lists `count` jobs of `action` that landed, and the page shows them landed. */
async function landed(action, count) {
  await eventually(`${String(count)} ${action} jobs landed`, async () => {
    const jobs = (await funding()).jobs.filter((job) => job.action === action);
    const failed = jobs.filter((job) => job.state === "failed");
    check(failed.length === 0, `a ${action} failed: ${failed.map((job) => job.detail).join("; ")}`);
    return jobs.filter((job) => job.state === "landed").length === count ? true : jobs.map((job) => job.state);
  });
  const rows = page.getByRole("region", { name: "Funding actions" }).getByRole("row").filter({ hasText: action }).filter({ hasText: "Landed" });
  await eventually(`the page shows ${String(count)} ${action} jobs landed`, async () => (await rows.count()) === count || (await rows.count()));
}

try {
  await page.goto(url);
  await page.getByLabel("Password").fill(password);
  await page.keyboard.press("Enter");
  await page.getByRole("heading", { level: 1, name: "Accounts" }).waitFor();
  await page.getByRole("link", { name: "Funding" }).click();
  await page.getByRole("heading", { level: 1, name: "Funding" }).waitFor();
  await page.getByRole("region", { name: "Funding of each account" }).waitFor();
  for (const address of accounts) {
    await page.getByRole("checkbox", { name: `Select ${address}` }).waitFor();
  }

  if (phase === "prepare" || phase === "all") {
    // Both accounts declare on one confirmation; nothing is sent until the statement is confirmed.
    await choose(accounts);
    await page.getByRole("radio", { name: /Professional client/ }).click();
    check(await page.getByRole("button", { name: "Declare" }).isDisabled(), "declare is offered before the statement is confirmed");
    await page.getByRole("checkbox", { name: /I have read the statement and confirm it for 2 selected accounts/ }).click();
    await page.getByRole("button", { name: "Declare" }).click();
    await landed("declare", 2);
    for (const address of accounts) {
      await eventually(`${address} shows as declared`, async () => (await figures(address)).declared === "professional");
    }

    // The first deposits from its own wallet, the second from the funding wallet.
    const before = await Promise.all(accounts.map(figures));
    await choose([accounts[0]]);
    await page.getByLabel("Amount of USDC for each account").fill(deposit);
    await page.getByLabel("From", { exact: true }).selectOption("account");
    await page.getByRole("button", { name: "Deposit" }).click();
    await landed("deposit", 1);
    await choose([accounts[1]]);
    await page.getByLabel("From", { exact: true }).selectOption("funding");
    await page.getByRole("button", { name: "Deposit" }).click();
    await landed("deposit", 2);
    for (const [index, address] of accounts.entries()) {
      await eventually(`${address}'s balance shows the deposit`, async () => {
        const now = await figures(address);
        return Number(now.balance ?? "0") === Number(before[index].balance ?? "0") + Number(deposit) || now.balance;
      });
    }
    const funded = await figures(accounts[0]);
    check(Number(funded.wallet_usdc) === Number(before[0].wallet_usdc) - Number(deposit), "the first account's wallet did not pay its own deposit");
    check(Number((await figures(accounts[1])).wallet_usdc) === Number(before[1].wallet_usdc), "the second account's own wallet paid a deposit the funding wallet made");
  }

  if (phase === "settle" || phase === "all") {
    const before = await Promise.all(accounts.map(figures));
    await choose(accounts);
    await page.getByRole("button", { name: "Claim" }).click();
    await landed("claim", 2);
    for (const [index, address] of accounts.entries()) {
      const now = await figures(address);
      check(Number(now.wallet_monkey) > Number(before[index].wallet_monkey), `${address} holds no more MONKEY after its claim`);
    }
    await page.getByLabel(/Amount of USDC from each account/).fill(withdrawal);
    await page.getByRole("button", { name: "Withdraw" }).click();
    await landed("withdraw", 2);
    for (const [index, address] of accounts.entries()) {
      await eventually(`${address}'s balance shows the withdrawal`, async () => {
        const now = await figures(address);
        return Number(now.balance) === Number(before[index].balance) - Number(withdrawal) || now.balance;
      });
      const now = await figures(address);
      check(Number(now.wallet_usdc) === Number(before[index].wallet_usdc) + Number(withdrawal), `${address}'s own wallet was not paid`);
    }
  }

  check(problems.length === 0, `page errors: ${problems.join("; ")}`);
  process.stdout.write(`funding browser test passed (${phase})\n`);
} finally {
  await browser.close();
}
