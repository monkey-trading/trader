/** The console's page frame and its pages. */
import { useEffect, useState } from "react";

import { ActionError, BUTTON_CLASS } from "@/components/ActionButton";
import { useServices, useSession } from "@/components/app-context";
import { ConnectionStatus } from "@/components/ConnectionStatus";
import { Link } from "@/components/Link";
import { useRoute } from "@/components/use-route";
import { AccountPage } from "@/pages/AccountPage";
import { LoginPage } from "@/pages/LoginPage";
import { OverviewPage } from "@/pages/OverviewPage";

/** How long after the bot did not answer the session check it is asked again. */
const SESSION_RETRY_MS = 3000;

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
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => live.start(), [live]);
  return (
    <>
      <div className="flex flex-wrap items-center justify-between gap-2 pb-4">
        <ConnectionStatus />
        <div className="text-right">
          <button
            type="button"
            className={BUTTON_CLASS}
            onClick={() => {
              setFailure(null);
              void session.logout().then((result) => {
                if (result.status === "failed") {
                  setFailure(result.message);
                }
              });
            }}
          >
            Sign out
          </button>
          <ActionError error={failure === null ? null : `The bot did not end the session. ${failure}`} />
        </div>
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
  // An unreachable bot is asked again, so the page comes up on its own once it answers.
  useEffect(() => {
    if (status !== "unreachable") {
      return undefined;
    }
    const retry = setTimeout(() => {
      void session.check();
    }, SESSION_RETRY_MS);
    return () => {
      clearTimeout(retry);
    };
  }, [session, status]);
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
        {status === "unreachable" ? <p role="alert" className="text-loss">The bot did not answer. Trying again…</p> : null}
        {status === "anonymous" ? <LoginPage /> : null}
        {status === "authenticated" ? <SignedIn /> : null}
      </main>
      <footer className="p-4 text-sm text-muted">Licensed under the Apache License, Version 2.0.</footer>
    </>
  );
}
