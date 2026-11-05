// A stand-in for the bot's console server: serves dist/ and the console API over plain HTTP on loopback, for the
// console's browser test and for `npm run dev` before a bot is at hand. Prints its URL on one line.
import { Buffer } from "node:buffer";
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { createModel, fills, flatten, overview, tick, view } from "./model.mjs";
import { accept } from "./websocket.mjs";

const PASSWORD = process.env.STUB_PASSWORD ?? "stub-password";
const TICK_MS = Number(process.env.STUB_TICK_MS ?? 1000);
const DIST = fileURLToPath(new URL("../dist", import.meta.url));
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml" };
const SECURITY_HEADERS = {
  "Content-Security-Policy":
    "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
};

const model = createModel(() => Date.now());
const sessions = new Set();
const feeds = new Set();
let failedLogins = 0;

function send(response, status, body, headers = {}) {
  response.writeHead(status, { "Content-Type": "application/json", ...SECURITY_HEADERS, ...headers });
  response.end(JSON.stringify(body));
}

async function readJson(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return null;
  }
}

function signedIn(request) {
  const match = /(?:^|; )session=([0-9a-f]+)/.exec(request.headers.cookie ?? "");
  return match !== null && sessions.has(match[1]);
}

async function login(request, response) {
  if (failedLogins >= 5) {
    return send(response, 429, { error: "too many attempts; try again later", code: "rate_limited" }, { "Retry-After": "30" });
  }
  const body = await readJson(request);
  if (body?.password !== PASSWORD) {
    failedLogins += 1;
    return send(response, 401, { error: "wrong password" });
  }
  failedLogins = 0;
  const token = randomBytes(16).toString("hex");
  sessions.add(token);
  response.writeHead(204, { ...SECURITY_HEADERS, "Set-Cookie": `session=${token}; HttpOnly; SameSite=Strict; Path=/` });
  return response.end();
}

function broadcast(messages) {
  for (const feed of feeds) {
    for (const message of messages) {
      feed.send(JSON.stringify(message));
    }
  }
}

/** Pushes what a control changed, as the bot's feed does. */
function pushChanges(item, withFills) {
  const messages = [{ type: "account", account: view(model, item) }, { type: "totals", totals: overview(model).totals }];
  if (withFills) {
    messages.push({ type: "fills", address: item.address, fills: item.fills });
  }
  broadcast(messages);
}

function noContent(response) {
  response.writeHead(204, SECURITY_HEADERS);
  response.end();
}

async function control(request, response, parts) {
  const [address, ...rest] = parts;
  const item = model.accounts.find((candidate) => candidate.address === address.toLowerCase());
  if (item === undefined) {
    return send(response, 404, { error: "no such account" });
  }
  const [what, id, action] = rest;
  if (request.method === "GET" && what === undefined) {
    return send(response, 200, view(model, item, true));
  }
  if (request.method === "GET" && what === "fills") {
    return send(response, 200, fills(item));
  }
  const body = await readJson(request);
  let flattened = false;
  if (request.method === "POST" && what === "flatten") {
    flatten(model, item);
    flattened = true;
  } else if (request.method === "POST" && (what === "start" || what === "stop")) {
    item.state = what === "start" ? "running" : "stopped";
  } else if (what === "strategies") {
    if (Number(id) !== item.strategy.id) {
      return send(response, 404, { error: "no such strategy" });
    }
    if (request.method === "POST" && item.blocked !== undefined && action === "start") {
      return send(response, 422, { error: `cannot start: ${item.blocked}` });
    }
    if (request.method === "POST") {
      item.strategy.running = action === "start";
    } else {
      if (body?.kind !== item.strategy.kind) {
        return send(response, 422, { error: "the parameters are for another kind of strategy", code: "kind_mismatch" });
      }
      const bounds = item.strategy.bounds;
      const names = Object.keys(item.strategy.params).filter((name) => name !== "kind");
      for (const name of names) {
        const value = Number(body?.[name]);
        if (bounds === null || Number.isNaN(value) || value < Number(bounds[name][0]) || value > Number(bounds[name][1])) {
          return send(response, 422, { error: `${name} is outside its bounds`, code: "out_of_bounds" });
        }
      }
      if (body.fast >= body.slow) {
        return send(response, 422, { error: "fast must be below slow" });
      }
      // The bot builds a new strategy for new parameters, which warms up again from the marks history.
      item.strategy.warmed_up = false;
      item.strategy.params = { kind: item.strategy.kind, ...Object.fromEntries(names.map((name) => [name, body[name]])) };
    }
  } else {
    return send(response, 404, { error: "not found" });
  }
  pushChanges(item, flattened);
  return noContent(response);
}

async function api(request, response, url) {
  if (request.method !== "GET" && request.headers.origin !== `http://${request.headers.host}`) {
    return send(response, 403, { error: "foreign origin", code: "foreign_origin" });
  }
  if (url.pathname === "/api/login" && request.method === "POST") {
    return login(request, response);
  }
  if (!signedIn(request)) {
    return send(response, 401, { error: "log in first", code: "session_required" });
  }
  const parts = url.pathname.split("/").slice(2).map(decodeURIComponent);
  if (parts[0] === "session") {
    return noContent(response);
  }
  if (parts[0] === "logout") {
    sessions.clear();
    response.writeHead(204, { ...SECURITY_HEADERS, "Set-Cookie": "session=; Max-Age=0; Path=/" });
    return response.end();
  }
  if (parts[0] === "accounts" && parts.length === 1) {
    return send(response, 200, overview(model));
  }
  if (parts[0] === "accounts") {
    return control(request, response, parts.slice(1));
  }
  return send(response, 404, { error: "not found" });
}

async function files(response, pathname) {
  const wanted = normalize(join(DIST, pathname === "/" ? "index.html" : pathname));
  const file = wanted.startsWith(DIST) ? wanted : join(DIST, "index.html");
  const body = await readFile(file).catch(() => readFile(join(DIST, "index.html")));
  const type = TYPES[extname(file)] ?? "text/html";
  response.writeHead(200, { "Content-Type": type, ...SECURITY_HEADERS });
  response.end(body);
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", "http://stub");
  const handled = url.pathname.startsWith("/api/") ? api(request, response, url) : files(response, url.pathname);
  handled.catch(() => {
    send(response, 500, { error: "stub failure" });
  });
});

server.on("upgrade", (request, socket) => {
  if (!signedIn(request) || request.url !== "/api/live") {
    socket.end("HTTP/1.1 401 Unauthorized\r\n\r\n");
    return;
  }
  const feed = accept(request, socket, () => feeds.delete(feed));
  feeds.add(feed);
  feed.send(JSON.stringify({ type: "snapshot", ...overview(model) }));
});

setInterval(() => {
  broadcast(tick(model));
}, TICK_MS).unref();

server.listen(Number(process.env.PORT ?? 0), "127.0.0.1", () => {
  const { port } = server.address();
  process.stdout.write(`stub console on http://127.0.0.1:${String(port)} (password: ${PASSWORD})\n`);
});
