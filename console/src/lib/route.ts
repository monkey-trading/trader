/** The console's client-side routes. */

/** A page of the console. */
export type Route =
  | { readonly name: "overview" }
  | { readonly name: "funding" }
  | { readonly name: "account"; readonly address: string }
  | { readonly name: "not-found" };

/** The path of the funding page. */
export const FUNDING_PATH = "/funding";

const ADDRESS = /^0x[0-9a-fA-F]{40}$/;

/** Maps a URL path to its page; a path that names no page, or a malformed address, is `not-found`. */
export function parseRoute(pathname: string): Route {
  if (pathname === "/") {
    return { name: "overview" };
  }
  if (pathname === FUNDING_PATH) {
    return { name: "funding" };
  }
  const match = /^\/accounts\/([^/]+)\/?$/.exec(pathname);
  const address = match?.[1];
  return address !== undefined && ADDRESS.test(address) ? { name: "account", address } : { name: "not-found" };
}

/** The path of an account's page. */
export function accountPath(address: string): string {
  return `/accounts/${address}`;
}
