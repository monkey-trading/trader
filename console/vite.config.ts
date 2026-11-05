import { fileURLToPath } from "node:url";

import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// `base: "/"`: the bot serves the console from its root. The dev server proxies the API and the feed to a bot
// (or to `npm run stub`) at CONSOLE_BOT_URL, which defaults to the bot's console address; a bot's self-signed
// certificate is accepted there because the dev server only ever talks to loopback.
const bot = process.env["CONSOLE_BOT_URL"] ?? "https://127.0.0.1:8443";

export default defineConfig({
  base: "/",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: {
    host: "127.0.0.1",
    proxy: { "/api": { target: bot, ws: true, secure: false, changeOrigin: false } },
  },
  build: { outDir: "dist" },
});
