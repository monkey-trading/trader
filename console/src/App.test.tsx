/** Tests of the console's page frame. */
import { render, screen } from "@testing-library/react";

import { App } from "@/App";

describe("App", () => {
  it("frames the page with a skip link to the main region and one h1", () => {
    render(<App />);

    expect(screen.getByRole("link", { name: "Skip to content" })).toHaveAttribute("href", "#main");
    expect(screen.getByRole("main")).toHaveAttribute("id", "main");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Console");
  });

  it("says the console has no bot connection", () => {
    render(<App />);

    expect(screen.getByText("The console is not connected to a bot yet.")).toBeInTheDocument();
  });
});
