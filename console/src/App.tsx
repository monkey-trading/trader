/** The console's page frame. */

/** The console: a frame with a skip link, a header, the main region, and a footer. */
export function App() {
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
      <main id="main" tabIndex={-1} className="p-4">
        <h1 className="text-2xl font-semibold">Console</h1>
        <p className="text-muted">The console is not connected to a bot yet.</p>
      </main>
      <footer className="p-4 text-sm text-muted">Licensed under the Apache License, Version 2.0.</footer>
    </>
  );
}
