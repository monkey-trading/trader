// The stand-in's funding side: wallets, declarations and operations that move through their stages on a
// timer, mutating the same accounts the console API serves. Nothing here holds a key or sends a transaction.
import { randomBytes } from "node:crypto";
import process from "node:process";

import { text, units } from "./model.mjs";

const STEP_MS = Number(process.env.STUB_STEP_MS ?? 400);
const MAX_OPERATIONS = 50;
const MINIMUM_DEPOSIT = units("10");
const STATEMENTS = {
  professional: "I attest that I am a professional client.",
  eligible_client: "I attest that I am an eligible client.",
};
const STAGES = {
  declare: ["Signing the declaration", "Waiting for the node to record it"],
  deposit: ["Approving USDC", "Depositing", "Waiting for the credit in the log"],
  claim: ["Sending the claim"],
  withdraw: ["Waiting in the withdrawal queue", "Submitting the authorization"],
};

/** Wallets and declarations for the model's accounts, and the operations in flight. */
export function createFunding(model) {
  const wallets = new Map(
    model.accounts.map((item, index) => [item.address, { usdc: units(["5000", "800", "100"][index] ?? "0"), eth: units("0.5"), declared: null }]),
  );
  return { wallets, operations: [], counter: 0, model };
}

/** The funding read, as the console parses it. */
export function read(funding) {
  const { model } = funding;
  return {
    accounts: model.accounts.map((item) => {
      const wallet = funding.wallets.get(item.address);
      return {
        address: item.address,
        name: item.name,
        declared: wallet.declared,
        walletUsdc: text(wallet.usdc),
        walletEth: text(wallet.eth),
        balance: wallet.declared === null && item.balance === 0n ? null : text(item.balance),
        unclaimedMonkey: text(item.earned - item.claimed),
        queued: [],
        unpaid: [],
      };
    }),
    declarations: Object.entries(STATEMENTS).map(([name, statement]) => ({ class: name, statement })),
    gasPayer: null,
    operations: funding.operations.map((op) => ({
      id: op.id, kind: op.kind, address: op.address, state: op.state, stage: op.stage, txHash: op.txHash, reason: op.reason,
    })),
  };
}

function fail(operation, reason) {
  operation.state = "failed";
  operation.reason = reason;
}

/** Applies the effect of an operation's last stage, or fails it the way the vault would. */
function finish(funding, operation, amount) {
  const item = funding.model.accounts.find((candidate) => candidate.address === operation.address);
  const wallet = funding.wallets.get(operation.address);
  if (operation.kind === "declare") {
    wallet.declared = operation.declaration;
  } else if (operation.kind === "deposit") {
    wallet.usdc -= amount;
    item.balance += amount;
  } else if (operation.kind === "claim") {
    item.claimed = item.earned;
  } else {
    item.balance -= amount;
    wallet.usdc += amount;
  }
  operation.state = "landed";
  operation.stage = "Landed";
}

/** What would make the transaction revert, as its reason, or null. */
function revertReason(funding, operation, amount) {
  const item = funding.model.accounts.find((candidate) => candidate.address === operation.address);
  const wallet = funding.wallets.get(operation.address);
  if (operation.kind === "deposit" && amount < MINIMUM_DEPOSIT) {
    return "Vault: deposit below the minimum of 10 USDC";
  }
  if (operation.kind === "deposit" && amount > wallet.usdc) {
    return "ERC20: transfer amount exceeds balance";
  }
  if (operation.kind === "withdraw" && amount > item.balance) {
    return "the withdrawal is more than the account's balance";
  }
  return null;
}

/** Starts one operation and walks it through its stages. */
function start(funding, kind, address, amount, extra = {}) {
  funding.counter += 1;
  const stages = STAGES[kind];
  const operation = { id: `op-${funding.counter}`, kind, address, state: "pending", stage: stages[0], txHash: null, reason: null, ...extra };
  funding.operations = [operation, ...funding.operations].slice(0, MAX_OPERATIONS);
  let step = 0;
  const next = () => {
    step += 1;
    if (step === 1 && kind !== "declare") {
      operation.txHash = `0x${randomBytes(32).toString("hex")}`;
    }
    const reason = step === 1 ? revertReason(funding, operation, amount) : null;
    if (reason !== null) {
      fail(operation, reason);
    } else if (step >= stages.length) {
      finish(funding, operation, amount);
    } else {
      operation.stage = stages[step];
      setTimeout(next, STEP_MS).unref();
    }
  };
  setTimeout(next, STEP_MS).unref();
}

/** The accounts a request names, or the refusal for the first it cannot act on. */
function targets(funding, body, kind) {
  const names = Array.isArray(body?.accounts) ? body.accounts : [];
  if (names.length === 0) {
    return { error: "name at least one account" };
  }
  for (const address of names) {
    if (!funding.wallets.has(address)) {
      return { error: `no such account: ${String(address).slice(0, 42)}` };
    }
    if (funding.operations.some((op) => op.kind === kind && op.address === address && op.state === "pending")) {
      return { error: `${address} already has a ${kind} in flight` };
    }
  }
  return { addresses: names };
}

/** Handles a funding action; returns `{ status, body }` for the server to send. */
export function act(funding, kind, body) {
  const found = targets(funding, body, kind);
  if (found.error !== undefined) {
    return { status: 409, body: { error: found.error } };
  }
  let amount = 0n;
  if (kind === "deposit" || kind === "withdraw") {
    const typed = String(body.amount);
    amount = /^\d{1,9}(?:\.\d{1,6})?$/.test(typed) ? units(typed) : 0n;
    if (amount === 0n) {
      return { status: 400, body: { error: "amount must be a positive USDC amount with at most six decimals" } };
    }
  }
  if (kind === "declare") {
    if (STATEMENTS[body.class] === undefined || STATEMENTS[body.class] !== body.statement) {
      return { status: 400, body: { error: "the statement is not the one the bot offers for that class" } };
    }
    const done = found.addresses.find((address) => funding.wallets.get(address).declared !== null);
    if (done !== undefined) {
      return { status: 409, body: { error: `${done} has declared already` } };
    }
  }
  for (const address of found.addresses) {
    start(funding, kind, address, amount, kind === "declare" ? { declaration: body.class } : {});
  }
  return { status: 200, body: read(funding) };
}
