/** The `Origin` the dev server's proxy sends to the bot for a request it received. */

/** The host names the dev server answers on, which is what its own page is served from. */
const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]"]);

/**
 * The bot refuses a state-changing request whose `Origin` is not its own `https://` address, but the dev server's
 * page is served over plain HTTP, so the proxy sends the bot's origin for the page's own requests. A request
 * from any other page, whose `Origin` is not the `http://` address it was sent to, keeps its `Origin` for the
 * bot to refuse. `origin` and `host` are the request's headers as received; a request with no `Origin` has none.
 */
export function proxiedOrigin(origin: string | undefined, host: string | undefined, botOrigin: string): string | undefined {
  if (origin === undefined || host === undefined) {
    return origin;
  }
  const name = host.startsWith("[") ? host.slice(0, host.indexOf("]") + 1) : host.split(":")[0];
  return origin === `http://${host}` && name !== undefined && LOOPBACK.has(name) ? botOrigin : origin;
}
