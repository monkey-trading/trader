/** Tests of the account page before the bot has read the account. */
import { screen } from "@testing-library/react";

import { overviewBody, totalsBody, viewBody } from "@/api/fixtures";
import { ADDRESS, mount, stubApi } from "@/test-harness";

describe("AccountPage before the bot has read the account", () => {
  it("does not say the account has no fills when the bot answers an empty list for a history it has not read", async () => {
    const api = stubApi(overviewBody([viewBody({ figures: null })], totalsBody()));
    api.fills.mockResolvedValue({ status: "ok", value: [] });
    mount(api, `/accounts/${ADDRESS}`);

    expect(await screen.findAllByText("Waiting for the bot to read this account.")).toHaveLength(3);
    expect(screen.queryByText("No fills yet.")).not.toBeInTheDocument();
    expect(api.fills).not.toHaveBeenCalled();
  });

  it("does not show fills the feed brings before the bot has read the account as a read history", async () => {
    const api = stubApi(overviewBody([viewBody({ figures: null })], totalsBody()));
    const { feeds } = mount(api, `/accounts/${ADDRESS}`);
    await screen.findAllByText("Waiting for the bot to read this account.");

    feeds[0]?.onMessage(JSON.stringify({ type: "fills", address: ADDRESS, fills: [] }));

    expect(await screen.findAllByText("Waiting for the bot to read this account.")).toHaveLength(3);
    expect(screen.queryByText("No fills yet.")).not.toBeInTheDocument();
  });
});
