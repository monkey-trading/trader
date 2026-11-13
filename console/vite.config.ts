import type { ClientRequest, IncomingMessage } from "node:http";
import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

import { proxiedOrigin } from "./src/lib/dev-origin";

// `base: "/"`: the bot serves the console from its root. The dev server proxies the API and the feed to a bot
// (or to `npm run stub`) at CONSOLE_BOT_URL, which defaults to the bot's console address; a bot's self-signed
// certificate is accepted there because the dev server only ever talks to loopback. The bot refuses a state-changing
// request or a feed whose `Origin` is not its own `https://` address, so the proxy sends the bot's origin as the
// request's: the page itself is served over plain HTTP from the dev server. Only the dev server's own page gets
// that; a request from any other page keeps its `Origin`, and the bot refuses it as a foreign origin.
const bot = process.env["CONSOLE_BOT_URL"] ?? "https://127.0.0.1:8443";
const botOrigin = new URL(bot).origin;

export default defineConfig({
  base: "/",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: {
    host: "127.0.0.1",
    proxy: {
      "/api": {
        target: bot,
        ws: true,
        secure: false,
        changeOrigin: true,
        configure: (proxy) => {
          const asBotOrigin = (proxied: ClientRequest, request: IncomingMessage) => {
            const { origin, host } = request.headers;
            if (origin !== undefined) {
              proxied.setHeader("origin", proxiedOrigin(origin, host, botOrigin) ?? origin);
            }
          };
          proxy.on("proxyReq", asBotOrigin);
          proxy.on("proxyReqWs", asBotOrigin);
        },
      },
    },
  },
  build: { outDir: "dist" },
});
