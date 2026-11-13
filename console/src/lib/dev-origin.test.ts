import { proxiedOrigin } from "./dev-origin";

const BOT = "https://127.0.0.1:8443";

describe("proxiedOrigin", () => {
  it("sends the bot's origin for the dev server's own page", () => {
    expect(proxiedOrigin("http://127.0.0.1:5173", "127.0.0.1:5173", BOT)).toBe(BOT);
    expect(proxiedOrigin("http://localhost:5174", "localhost:5174", BOT)).toBe(BOT);
    expect(proxiedOrigin("http://[::1]:5173", "[::1]:5173", BOT)).toBe(BOT);
  });

  it("leaves the origin of a page served from another port for the bot to refuse", () => {
    expect(proxiedOrigin("http://127.0.0.1:9999", "127.0.0.1:5173", BOT)).toBe("http://127.0.0.1:9999");
    expect(proxiedOrigin("http://localhost:5173", "127.0.0.1:5173", BOT)).toBe("http://localhost:5173");
    expect(proxiedOrigin("null", "127.0.0.1:5173", BOT)).toBe("null");
  });

  it("leaves the origin of a page on another host, even one that names the dev server as its host", () => {
    expect(proxiedOrigin("http://example.test:5173", "example.test:5173", BOT)).toBe("http://example.test:5173");
  });

  it("adds no origin to a request that has none", () => {
    expect(proxiedOrigin(undefined, "127.0.0.1:5173", BOT)).toBeUndefined();
    expect(proxiedOrigin("http://127.0.0.1:5173", undefined, BOT)).toBe("http://127.0.0.1:5173");
  });
});
