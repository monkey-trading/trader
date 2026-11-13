/** Bodies shaped as the bot's funding routes serve them, for the tests. */
import { ADDRESS } from "./fixtures";

/** A second account. */
export const OTHER = `0x${"22".repeat(20)}`;

export const PROFESSIONAL = "I attest that I am a professional client.";
export const ELIGIBLE = "I attest that I am an eligible client.";

/** One account's funding. */
export function fundingAccountBody(address = ADDRESS, overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    address,
    error: null,
    declared: null,
    wallet_eth: "1.25",
    wallet_usdc: "1500",
    wallet_monkey: "0",
    balance: null,
    unclaimed: "0",
    unclaimed_newest: "0",
    queued: [],
    unpaid: [],
    payouts_loaded: true,
    busy: false,
    reading: false,
    ...overrides,
  };
}

/** One job. */
export function jobBody(id: number, overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, action: "declare", account: ADDRESS, state: "running", detail: "", txs: [], ...overrides };
}

/** The funding read. */
export function fundingBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    enabled: true,
    funding_key: true,
    statements: { professional: PROFESSIONAL, eligible_client: ELIGIBLE },
    accounts: [fundingAccountBody(ADDRESS), fundingAccountBody(OTHER, { declared: "professional", balance: "1000" })],
    jobs: [],
    ...overrides,
  };
}
