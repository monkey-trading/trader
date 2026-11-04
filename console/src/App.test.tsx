/** Tests of the page frame, login, and navigation between the pages. */
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { ADDRESS, mount, stubApi } from "@/test-harness";

describe("App", () => {
  it("frames the page with a skip link to the main region and one h1", async () => {
    mount(stubApi());

    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main");
    expect(screen.getByRole("main")).toHaveAttribute("tabindex", "-1");
    expect(await screen.findByRole("heading", { level: 1, name: "Accounts" })).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });

  it("asks for the password when there is no session and shows a refusal at the field", async () => {
    const api = stubApi();
    api.session.mockResolvedValue({ status: "unauthenticated" });
    api.login.mockResolvedValueOnce({ status: "unauthenticated" });
    const user = userEvent.setup();
    mount(api);

    await user.type(await screen.findByLabelText("Password"), "wrong{Enter}");

    expect(api.login).toHaveBeenCalledExactlyOnceWith("wrong");
    expect(await screen.findByText("The password was not accepted.")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toHaveAttribute("aria-invalid", "true");

    await user.clear(screen.getByLabelText("Password"));
    await user.type(screen.getByLabelText("Password"), "right{Enter}");
    expect(await screen.findByRole("heading", { level: 1, name: "Accounts" })).toBeInTheDocument();
  });

  it("returns to the login page when a call says the session ended", async () => {
    const api = stubApi();
    api.accounts.mockResolvedValue({ status: "unauthenticated" });
    mount(api);

    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });

  it("shows every account's figures exactly as served, with totals", async () => {
    mount(stubApi());

    const table = await screen.findByRole("table", { name: "Accounts" });
    const [, row, total] = within(table).getAllByRole("row");
    expect(within(row ?? table).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "running", "10,100.50", "-3.25", "+100.75", "3,350.00%", "1", "12.50", "5.00",
    ]);
    expect(within(total ?? table).getAllByRole("cell").map((cell) => cell.textContent)).toEqual([
      "", "10,100.50", "-3.25", "+100.75", "—", "1", "12.50", "5.00",
    ]);
  });

  it("opens an account page from the overview and shows its tables and strategies", async () => {
    const user = userEvent.setup();
    mount(stubApi());

    await user.click(await screen.findByRole("link", { name: "alpha" }));

    expect(window.location.pathname).toBe(`/accounts/${ADDRESS}`);
    expect(await screen.findByRole("heading", { level: 1, name: "alpha" })).toHaveFocus();
    expect(screen.getByRole("table", { name: "Open positions" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Resting orders" })).toBeInTheDocument();
    expect(screen.getByRole("table", { name: "Recent fills" })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /^Equity now 10,100.50 USDC/ })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "SMA crossover on BTC-USD" })).toBeInTheDocument();
  });

  it("says when an account cannot be read, at the page", async () => {
    const api = stubApi();
    api.account.mockResolvedValue({ status: "refused", message: "not found", retryAfterSeconds: null });
    mount(api, `/accounts/${ADDRESS}`);

    expect(await screen.findByText("This account could not be read: not found")).toBeInTheDocument();
  });

  it("says the feed is down and that the figures may be old, and never claims live", async () => {
    const api = stubApi();
    const { feeds } = mount(api);
    await screen.findByRole("heading", { name: "Accounts" });
    expect(await screen.findByText("Live")).toBeInTheDocument();

    feeds[0]?.onClose();

    expect(await screen.findByText(/Disconnected\. Reconnecting/)).toBeInTheDocument();
    expect(screen.queryByText("Live")).not.toBeInTheDocument();
  });

  it("signs out through the bot and shows the login page", async () => {
    const api = stubApi();
    const user = userEvent.setup();
    mount(api);

    await user.click(await screen.findByRole("button", { name: "Sign out" }));

    await waitFor(() => { expect(api.logout).toHaveBeenCalledTimes(1); });
    expect(await screen.findByRole("heading", { level: 1, name: "Sign in" })).toBeInTheDocument();
  });
});
