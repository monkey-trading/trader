/** The console's page frame and its pages. */
import { useEffect } from "react";

import { BUTTON_CLASS } from "@/components/ActionButton";
import { useServices, useSession } from "@/components/app-context";
import { ConnectionStatus } from "@/components/ConnectionStatus";
import { Link } from "@/components/Link";
import { useRoute } from "@/components/use-route";
import { AccountPage } from "@/pages/AccountPage";
import { LoginPage } from "@/pages/LoginPage";
import { OverviewPage } from "@/pages/OverviewPage";

/** The page the URL names, shown once signed in. */
function Pages() {
  const route = useRoute();
  switch (route.name) {
    case "overview":
      return <OverviewPage />;
    case "account":
      return <AccountPage key={route.address} address={route.address} />;
    case "not-found":
      return (
        <>
          <h1 className="mb-4 text-2xl font-semibold">Page not found</h1>
          <Link to="/">All accounts</Link>
        </>
      );
  }
}

/** Opens the live feed while signed in, and offers sign out. */
function SignedIn() {
  const { live, session } = useServices();
  useEffect(() => live.start(), [live]);
  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-2 pb-4">
        <ConnectionStatus />
        <button
          type="button"
          className={BUTTON_CLASS}
          onClick={() => {
            void session.logout();
          }}
        >
          Sign out
        </button>
      </div>
      <Pages />
    </>
  );
}

/** The console: a frame with a skip link, a header, the main region, and a footer. */
export function App() {
  const { session } = useServices();
  const status = useSession();
  useEffect(() => {
    void session.check();
  }, [session]);
  return (
    <>
      <a
        href="#main"
        className="sr-only focus-visible:not-sr-only focus-visible:absolute focus-visible:top-2 focus-visible:left-2 focus-visible:z-10 focus-visible:bg-paper focus-visible:p-2 focus-visible:text-ink focus-visible:outline-2"
      >
        Skip to content
      </a>
      <header className="p-4">
        <p className="font-semibold">monkeytrading trader</p>
      </header>
      <main id="main" tabIndex={-1} className="p-4 outline-none">
        {status === "checking" ? <p className="text-muted">Checking the session…</p> : null}
        {status === "anonymous" ? <LoginPage /> : null}
        {status === "authenticated" ? <SignedIn /> : null}
      </main>
      <footer className="p-4 text-sm text-muted">Licensed under the Apache License, Version 2.0.</footer>
    </>
  );
}
