import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `base: "/"`: the bot serves the console from its root. The dev server proxies the API and the feed to a bot
// (or to `npm run stub`) at CONSOLE_BOT_URL, which defaults to the bot's console address; a bot's self-signed
// certificate is accepted there because the dev server only ever talks to loopback. The bot refuses a state-changing
// request or a feed whose `Origin` is not its own `https://` address, so the proxy sends the bot's origin as the
// request's: the page itself is served over plain HTTP from the dev server.
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
          const asBotOrigin = (request: { hasHeader(name: string): boolean; setHeader(name: string, value: string): unknown }) => {
            if (request.hasHeader("origin")) {
              request.setHeader("origin", botOrigin);
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
