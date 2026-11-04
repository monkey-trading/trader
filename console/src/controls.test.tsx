/** Tests of the account page's controls: what the user does, and what shows when the bot refuses. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Outcome } from "@/api/client";
import type { AccountRead } from "@/api/types";
import { ADDRESS, mount, stubApi } from "@/test-harness";

const PAGE = `/accounts/${ADDRESS}`;

/** A promise the test settles by hand, to hold a call in flight. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("strategy controls", () => {
  it("stops a strategy once however often the button is pressed, and shows it pending", async () => {
    const api = stubApi();
    const held = deferred<Outcome<AccountRead>>();
    api.setStrategyRunning.mockReturnValue(held.promise);
    const user = userEvent.setup();
    mount(api, PAGE);

    const stop = await screen.findByRole("button", { name: "Stop strategy" });
    await user.dblClick(stop);

    expect(api.setStrategyRunning).toHaveBeenCalledExactlyOnceWith(ADDRESS, "alpha-sma", false);
    expect(await screen.findByRole("button", { name: "Stopping…" })).toBeDisabled();
  });

  it("shows the bot's refusal under the control that caused it and nowhere else", async () => {
    const api = stubApi();
    api.setStrategyRunning.mockResolvedValue({
      status: "refused", message: "cannot start: the account is halted by its daily loss limit", retryAfterSeconds: null,
    });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Stop strategy" }));

    const card = screen.getByRole("region", { name: "SMA crossover on BTC-USD" });
    expect(await within(card).findByText("Failed: cannot start: the account is halted by its daily loss limit")).toBeInTheDocument();
    expect(screen.getAllByText(/^Failed:/)).toHaveLength(1);
    expect(within(card).getByRole("button", { name: "Stop strategy" })).toBeEnabled();
  });
});

describe("account controls", () => {
  it("stops the account and shows the state the bot answers with", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Stop account" }));

    expect(api.setAccountRunning).toHaveBeenCalledExactlyOnceWith(ADDRESS, false);
    expect(await screen.findByRole("button", { name: "Start account" })).toBeInTheDocument();
  });

  it("flattens only after a second press, which a keyboard reaches and can cancel", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Flatten account" }));
    expect(api.flatten).not.toHaveBeenCalled();
    await waitFor(() => { expect(screen.getByRole("button", { name: "Confirm flatten" })).toHaveFocus(); });

    await user.keyboard("{Tab}{Enter}");
    expect(screen.queryByRole("button", { name: "Confirm flatten" })).not.toBeInTheDocument();
    await waitFor(() => { expect(screen.getByRole("button", { name: "Flatten account" })).toHaveFocus(); });
    expect(api.flatten).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Flatten account" }));
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));

    expect(api.flatten).toHaveBeenCalledExactlyOnceWith(ADDRESS);
    expect(await screen.findByText("No open positions.")).toBeInTheDocument();
  });

  it("shows a failed flatten at the control", async () => {
    const api = stubApi();
    api.flatten.mockResolvedValue({ status: "unavailable", message: "The bot did not answer." });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Flatten account" }));
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));

    expect(await screen.findByText("Failed: The bot did not answer.")).toBeInTheDocument();
  });
});

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
  });

  it("sends a valid value as a number and takes the bot's answer", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    const field = await screen.findByLabelText("fast");
    await user.clear(field);
    await user.type(field, "8{Enter}");

    expect(api.setParams).toHaveBeenCalledExactlyOnceWith(ADDRESS, "alpha-sma", { fast: 8 });
  });

  it("shows the bot's refusal of parameters under the form", async () => {
    const api = stubApi();
    api.setParams.mockResolvedValue({
      status: "refused", message: "fast is outside 2 to 50", retryAfterSeconds: null,
    });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Save parameters" }));

    expect(await screen.findByText("Failed: fast is outside 2 to 50")).toBeInTheDocument();
  });
});
