# No unapproved deploys

Load on request before any command that changes a live environment, and before merging a PR.

**Rule**: Never deploy or otherwise mutate a live environment without approval for that action.

- Covers:
  - starting the bot against a production node, or placing, cancelling, or closing anything for an
    account that holds real funds;
  - sending a transaction on a public chain: a deposit, a MONKEY claim, a withdrawal, an approval,
    or a contract deployment;
  - declaring an account (`POST /v1/eligibility`) for a user who has not confirmed it;
  - writing, exporting, or moving keys, keystores, passwords, or the console's login;
  - pushing a release tag, publishing an image or binary, and changing repository settings,
    rulesets, Actions secrets, or variables, which can bypass the merge gate;
  - merging to `main`, and posting `merge-approved`, which allows the merge
    ([agent-create-pr](../skills/agent-create-pr/SKILL.md#merging)).
- A dev stack on loopback, with mock custody or a local Anvil chain and keys whose private keys the
  repository publishes, is not a live environment; anything holding real funds, or connected to a
  public chain, is one.
- Approval covers one action. A yes to deploy is not a yes to change its config, and a yes on
  testnet is not a yes on mainnet.
- When in doubt, propose the exact command and wait.
