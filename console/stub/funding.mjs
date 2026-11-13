// The funding routes of the stand-in console server: each account's funding in memory, and each action a job that
// runs for a moment and then lands or fails, as the bot's does. The bot's own rules (the statements, the refusals)
// are copied, not imported: this is a stand-in for the page's tests and `npm run stub`.
import process from "node:process";

const STATEMENTS = {
  professional: "I attest that I am a professional client.",
  eligible_client: "I attest that I am an eligible client.",
};
const JOB_MS = Number(process.env.STUB_JOB_MS ?? 600);

function account(address) {
  return {
    address, error: null, declared: null, wallet_eth: "1", wallet_usdc: "1500", wallet_monkey: "0", balance: null,
    unclaimed: "25", unclaimed_newest: "25", queued: [], unpaid: [], payouts_loaded: true, busy: false,
  };
}

/** The funding of `addresses`, and the jobs started so far. */
export function createFunding(addresses) {
  return { accounts: addresses.map(account), jobs: [], next: 1 };
}

export function read(funding) {
  return { enabled: true, funding_key: true, statements: STATEMENTS, accounts: funding.accounts, jobs: funding.jobs };
}

function refusal(status, code, error) {
  return { status, body: { error, code } };
}

/** Starts `action` for the accounts the body names: the answer, or a refusal that starts nothing. */
export function start(funding, action, body) {
  const addresses = [...new Set(body?.accounts ?? [])];
  if (addresses.length === 0) {
    return refusal(422, "bad_accounts", "name between one and 500 accounts");
  }
  if (action === "declare" && body.statement !== STATEMENTS[body.class]) {
    return refusal(422, "statement_mismatch", "the statement is not the class's");
  }
  if (action === "deposit" && !/^\d+(\.\d{1,6})?$/.test(String(body.amount)) ) {
    return refusal(422, "bad_amount", `${String(body.amount)} USDC is not above zero and within bounds`);
  }
  const items = [];
  for (const address of addresses) {
    const item = funding.accounts.find((candidate) => candidate.address === address);
    if (item === undefined) {
      return refusal(404, "no_account", `${address} is not one of the bot's accounts`);
    }
    if (item.busy) {
      return refusal(409, "busy", `${address} has an action that has not ended`);
    }
    items.push(item);
  }
  const jobs = items.map((item) => {
    item.busy = true;
    const job = { id: funding.next, action, account: item.address, state: "running", detail: "", txs: [] };
    funding.next += 1;
    funding.jobs.push(job);
    setTimeout(() => {
      land(item, job, body);
      item.busy = false;
    }, JOB_MS).unref();
    return { ...job };
  });
  return { status: 202, body: { jobs } };
}

/** Does what the action does to the account, and ends its job. */
function land(item, job, body) {
  job.state = "landed";
  const tx = `0x${String(job.id).padStart(64, "0")}`;
  switch (job.action) {
    case "declare":
      item.declared = body.class;
      job.detail = `declared ${body.class}`;
      break;
    case "deposit":
      item.balance = String(Number(item.balance ?? "0") + Number(body.amount));
      if (body.from !== "funding") {
        item.wallet_usdc = String(Number(item.wallet_usdc) - Number(body.amount));
      }
      job.detail = `${body.amount} USDC credited`;
      job.txs = [tx];
      break;
    case "claim":
      job.detail = item.unclaimed === "0" ? "no MONKEY to claim" : `claimed ${item.unclaimed} MONKEY`;
      job.txs = item.unclaimed === "0" ? [] : [tx];
      item.wallet_monkey = String(Number(item.wallet_monkey) + Number(item.unclaimed));
      item.unclaimed = "0";
      item.unclaimed_newest = "0";
      break;
    default:
      if (body.amount === undefined) {
        job.detail = "nothing to pay";
        break;
      }
      if (Number(body.amount) > Number(item.balance ?? "0")) {
        job.state = "failed";
        job.detail = "the account holds less than the amount";
        break;
      }
      item.balance = String(Number(item.balance) - Number(body.amount));
      item.wallet_usdc = String(Number(item.wallet_usdc) + Number(body.amount));
      job.detail = `${body.amount} USDC paid to the account`;
      job.txs = [tx];
  }
}
