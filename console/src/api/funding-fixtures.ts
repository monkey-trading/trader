/** Funding bodies shaped as the bot serves them, for the tests of the parser, store and page. */

export const ALPHA = `0x${"11".repeat(20)}`;
export const BETA = `0x${"22".repeat(20)}`;

/** One account's funding figures. */
export function fundingAccountBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    address: ALPHA,
    name: "alpha",
    declared: null,
    walletUsdc: "1000.5",
    walletEth: "0.25",
    balance: null,
    unclaimedMonkey: "0",
    queued: [],
    unpaid: [],
    ...overrides,
  };
}

/** The operation the bot lists for an action in flight. */
export function operationBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "op-1",
    kind: "deposit",
    address: ALPHA,
    state: "pending",
    stage: "Waiting for the credit in the log",
    txHash: `0x${"ab".repeat(32)}`,
    reason: null,
    ...overrides,
  };
}

/** The funding read: two accounts, both declarations on offer, no operations. */
export function fundingBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    accounts: [
      fundingAccountBody(),
      fundingAccountBody({ address: BETA, name: "beta", declared: "professional", balance: "250", unclaimedMonkey: "7.5" }),
    ],
    declarations: [
      { class: "professional", statement: "I attest that I am a professional client." },
      { class: "eligible_client", statement: "I attest that I am an eligible client." },
    ],
    gasPayer: null,
    operations: [],
    ...overrides,
  };
}
