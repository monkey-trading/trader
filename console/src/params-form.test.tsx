/** Tests of the strategy parameter form: its checks, what it sends, and what it says. */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { overviewBody, strategyBody, viewBody } from "@/api/fixtures";
import { ADDRESS, mount, stubApi } from "@/test-harness";

const PAGE = `/accounts/${ADDRESS}`;

describe("parameter form", () => {
  it("refuses a value outside the bounds at its field and sends nothing", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    const field = await screen.findByLabelText("fast");
    await user.clear(field);
    await user.type(field, "1");
    await user.click(screen.getByRole("button", { name: "Save parameters" }));

    expect(api.setParams).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a value from 2 to 50.")).toBeInTheDocument();
    expect(field).toHaveAttribute("aria-invalid", "true");
    // Focus moves to the field that is wrong, whose linked help line says why.
    await waitFor(() => { expect(field).toHaveFocus(); });
    expect(field).toHaveAccessibleDescription("Enter a value from 2 to 50.");
  });

  it("says a save worked, and a flatten, where the screen reader is", async () => {
    const api = stubApi();
    api.flatten.mockResolvedValue({ status: "ok", value: null });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Save parameters" }));
    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Flatten account" }));
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));
    expect(await screen.findByText("Done.")).toBeInTheDocument();
    // Focus stays on the status line, the confirm button having gone.
    await waitFor(() => { expect(screen.getByText("Done.")).toHaveFocus(); });
  });

  it("keeps saying a save worked after the saved values arrive and the form is made over", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    const { feeds } = mount(api, PAGE);

    const field = await screen.findByLabelText("fast");
    await user.clear(field);
    await user.type(field, "7");
    api.setParams.mockImplementation(() => {
      // The bot's push brings the new values while the save's answer is on its way.
      feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ strategies: [{ ...strategyBody(), params: { fast: 7, slow: 20 } }] }) }));
      return Promise.resolve({ status: "ok", value: null });
    });
    await user.click(screen.getByRole("button", { name: "Save parameters" }));

    expect(await screen.findByText("Saved.")).toBeInTheDocument();
    await waitFor(() => { expect(screen.getByLabelText("fast")).toHaveValue("7"); });
    expect(screen.getByText("Saved.")).toBeInTheDocument();
  });

  it("sends every parameter with the strategy's kind, the edited one changed", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    const field = await screen.findByLabelText("fast");
    await user.clear(field);
    await user.type(field, "8{Enter}");

    expect(api.setParams).toHaveBeenCalledExactlyOnceWith(ADDRESS, 0, { kind: "sma_crossover", fast: 8, slow: 20 });
  });

  it("shows the bot's refusal of the periods under the form", async () => {
    const api = stubApi();
    api.setParams.mockResolvedValue({ status: "refused", message: "fast must be below slow", retryAfterSeconds: null });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Save parameters" }));

    expect(await screen.findByText("Failed: fast must be below slow")).toBeInTheDocument();
  });

  it("takes a decimal parameter as text and sends it as a string beside the whole-number ones", async () => {
    const bollinger = {
      id: 0, kind: "bollinger_reversion", market: "BTC-USD", bar: "5m", running: true, warmed_up: true, target: null,
      params: { kind: "bollinger_reversion", period: 20, width: "1.5" },
      bounds: { period: [5, 100], width: ["0.5", "4"] },
    };
    const api = stubApi(overviewBody([viewBody({ strategies: [bollinger] })]));
    const user = userEvent.setup();
    mount(api, PAGE);

    const width = await screen.findByLabelText("width");
    expect(screen.getByRole("region", { name: "Bollinger band reversion on BTC-USD" })).toBeInTheDocument();
    await user.clear(width);
    await user.type(width, "4.5{Enter}");
    expect(api.setParams).not.toHaveBeenCalled();
    expect(screen.getByText("Enter a value from 0.5 to 4.")).toBeInTheDocument();

    await user.clear(width);
    await user.type(width, "2.25{Enter}");
    expect(api.setParams).toHaveBeenCalledExactlyOnceWith(ADDRESS, 0, { kind: "bollinger_reversion", period: 20, width: "2.25" });
  });

  it("shows the periods read-only when the config sets no bounds", async () => {
    const strategy = { ...strategyBody(), bounds: null };
    mount(stubApi(overviewBody([viewBody({ strategies: [strategy] })])), PAGE);

    expect(await screen.findByText(/fast 5, slow 20/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save parameters" })).not.toBeInTheDocument();
  });
});
