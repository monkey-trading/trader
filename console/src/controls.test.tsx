/** Tests of the account page's controls: what the user does, and what shows when the bot refuses. */
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { Outcome } from "@/api/client";
import { figuresBody, overviewBody, strategyBody, viewBody } from "@/api/fixtures";
import { readAccount } from "@/api/parsers";
import { ADDRESS, mount, stubApi } from "@/test-harness";

const PAGE = `/accounts/${ADDRESS}`;
const STOPPED = { status: "ok", value: readAccount(viewBody({ state: "stopped" })) } as const;

/** A promise the test settles by hand, to hold a call in flight. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("strategy controls", () => {
  it("stops a strategy once however often the button is pressed, and shows it pending", async () => {
    const api = stubApi();
    const held = deferred<Outcome<null>>();
    api.setStrategyRunning.mockReturnValue(held.promise);
    const user = userEvent.setup();
    mount(api, PAGE);

    const stop = await screen.findByRole("button", { name: "Stop strategy" });
    await user.dblClick(stop);

    expect(api.setStrategyRunning).toHaveBeenCalledExactlyOnceWith(ADDRESS, 0, false);
    expect(await screen.findByRole("button", { name: "Stopping…" })).toBeDisabled();
  });

  it("reads the account again after a stop and shows the state the bot gives", async () => {
    const api = stubApi();
    const stopped = readAccount(viewBody({ strategies: [{ ...strategyBody(), running: false }] }));
    const user = userEvent.setup();
    mount(api, PAGE);

    const stop = await screen.findByRole("button", { name: "Stop strategy" });
    api.account.mockResolvedValue({ status: "ok", value: stopped });
    await user.click(stop);

    expect(await screen.findByRole("button", { name: "Start strategy" })).toBeInTheDocument();
    expect(screen.getByText("Stopped")).toBeInTheDocument();
  });

  it("shows the bot's refusal under the control that caused it and nowhere else", async () => {
    const api = stubApi();
    api.setStrategyRunning.mockResolvedValue({
      status: "refused", message: "cannot start: the account is over its daily loss limit", retryAfterSeconds: null,
    });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Stop strategy" }));

    const card = screen.getByRole("region", { name: "SMA crossover on BTC-USD" });
    expect(await within(card).findByText("Failed: cannot start: the account is over its daily loss limit")).toBeInTheDocument();
    expect(screen.getAllByText(/^Failed:/)).toHaveLength(1);
    expect(within(card).getByRole("button", { name: "Stop strategy" })).toBeEnabled();
  });
});

describe("account controls", () => {
  it("stops the account and shows the state the bot gives", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    const stop = await screen.findByRole("button", { name: "Stop account" });
    api.account.mockResolvedValue(STOPPED);
    await user.click(stop);

    expect(api.setAccountRunning).toHaveBeenCalledExactlyOnceWith(ADDRESS, false);
    expect(await screen.findByRole("button", { name: "Start account" })).toBeInTheDocument();
  });

  it("offers no control to an account that is starting, flattening or halted", async () => {
    mount(stubApi(overviewBody([viewBody({ state: "halted", halt: "an answer was signed by another key" })])), PAGE);

    expect(await screen.findByText(/The account stopped on its own/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /account$/ })).not.toBeInTheDocument();
  });

  it("flattens only after a second press, which a keyboard reaches and can cancel", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    const flatten = await screen.findByRole("button", { name: "Flatten account" });
    api.account.mockResolvedValue({
      status: "ok",
      value: readAccount(viewBody({ state: "stopped", figures: figuresBody({ positions: [] }) })),
    });
    await user.click(flatten);
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

  it("does not flatten on a double-click: the second click lands on the confirm button the first brought up", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.dblClick(await screen.findByRole("button", { name: "Flatten account" }));
    expect(await screen.findByRole("button", { name: "Confirm flatten" })).toBeInTheDocument();
    expect(api.flatten).not.toHaveBeenCalled();
    await user.dblClick(screen.getByRole("button", { name: "Confirm flatten" }));
    expect(api.flatten).toHaveBeenCalledOnce();
  });

  it("keeps focus on the flatten's status line when the account reads flattening and then stopped", async () => {
    const api = stubApi();
    const held = deferred<Outcome<null>>();
    api.flatten.mockReturnValue(held.promise);
    const user = userEvent.setup();
    const { feeds } = mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Flatten account" }));
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));
    await waitFor(() => { expect(document.activeElement).toHaveAttribute("aria-live", "polite"); });
    const status = document.activeElement;
    feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "flattening" }) }));
    expect(await screen.findByText("The account is closing its positions.")).toBeInTheDocument();
    expect(status).toHaveFocus();
    feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "stopped" }) }));
    await waitFor(() => { expect(screen.getByRole("button", { name: "Start account" })).toBeInTheDocument(); });
    expect(status).toHaveFocus();
  });

  it("does not stop and start again on a double-click", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.dblClick(await screen.findByRole("button", { name: "Stop account" }));
    expect(api.setAccountRunning).toHaveBeenCalledExactlyOnceWith(ADDRESS, false);
  });

  it("shows a failed flatten at the control, including a timeout the bot says may still act", async () => {
    const api = stubApi();
    api.flatten.mockResolvedValue({
      status: "unavailable", message: "the account has not answered yet; it may still act on the request",
    });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Flatten account" }));
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));

    expect(await screen.findByText("Failed: the account has not answered yet; it may still act on the request")).toBeInTheDocument();
  });
});

describe("a flatten whose account changes state while it is out", () => {
  it("shows the failure or the warning where it was asked for, though the account reads flattening meanwhile", async () => {
    for (const [answer, shown] of [
      [{ status: "unavailable", message: "the flatten left a position open" }, "Failed: the flatten left a position open"],
      [{ status: "ok", value: { warning: "the stop was not saved" } }, "Done, with a warning: the stop was not saved"],
    ] as const) {
      const api = stubApi();
      const held = deferred<Outcome<{ warning: string } | null>>();
      api.flatten.mockReturnValue(held.promise);
      const user = userEvent.setup();
      const { feeds, unmount } = mount(api, PAGE);

      await user.click(await screen.findByRole("button", { name: "Flatten account" }));
      await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));
      // The bot publishes `flattening` as soon as the runner queues the flatten, before the answer.
      feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "flattening" }) }));
      expect(await screen.findByText("The account is closing its positions.")).toBeInTheDocument();
      feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody({ state: "stopped" }) }));
      await act(async () => {
        held.resolve(answer);
        await held.promise;
      });

      await waitFor(() => { expect(screen.getByText(shown)).toBeInTheDocument(); });
      unmount();
    }
  });
});

describe("what the bot says of a control", () => {
  it("shows a flatten whose stop was not saved as done with a warning, not as a failure, and reads the account", async () => {
    const api = stubApi();
    const message = "the account is flattened; the stop holds, but was not saved, so a restart may start it again";
    api.flatten.mockResolvedValue({ status: "ok", value: { warning: message } });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Flatten account" }));
    const accountReads = api.account.mock.calls.length;
    await user.click(await screen.findByRole("button", { name: "Confirm flatten" }));

    const warning = await screen.findByText(`Done, with a warning: ${message}`);
    expect(warning).toHaveClass("text-warn");
    expect(screen.queryByText(/^Failed:/)).not.toBeInTheDocument();
    await waitFor(() => { expect(api.account.mock.calls.length).toBeGreaterThan(accountReads); });
  });

  it("shows a start the bot refuses during a flatten at the strategy's start control", async () => {
    const api = stubApi(overviewBody([viewBody({ strategies: [{ ...strategyBody(), running: false }] })]));
    api.setStrategyRunning.mockResolvedValue({
      status: "refused", message: "the account is flattening; start it once it has", retryAfterSeconds: null,
    });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Start strategy" }));

    const card = screen.getByRole("region", { name: "SMA crossover on BTC-USD" });
    expect(await within(card).findByText("Failed: the account is flattening; start it once it has")).toBeInTheDocument();
  });

  it("shows a request the bot dropped as too slow or too large at the control that sent it", async () => {
    const api = stubApi();
    api.setParams.mockResolvedValue({ status: "refused", message: "The request was too large for the bot to take.", retryAfterSeconds: null });
    const user = userEvent.setup();
    mount(api, PAGE);

    await user.click(await screen.findByRole("button", { name: "Save parameters" }));

    expect(await screen.findByText("Failed: The request was too large for the bot to take.")).toBeInTheDocument();
  });

  it("shows a strategy as warming up while the bot has not warmed it, and drops the note once it has", async () => {
    const cold = { ...strategyBody(), warmed_up: false, target: null };
    const { feeds } = mount(stubApi(overviewBody([viewBody({ strategies: [cold] })])), PAGE);

    expect(await screen.findByText(/, warming up/)).toBeInTheDocument();

    feeds[0]?.onMessage(JSON.stringify({ type: "account", account: viewBody() }));
    await waitFor(() => { expect(screen.queryByText(/warming up/)).not.toBeInTheDocument(); });
  });
});
