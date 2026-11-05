/** Parsers that turn the bot's JSON bodies and feed messages into typed values. */
import { isDecimal } from "@/lib/decimal";
import {
  baseUnits, choice, count, decimal, flag, items, list, MAX_TEXT, nullableBaseUnits, nullableDecimal, nullableInteger,
  nullableText, object, ShapeError, text, type Fields,
} from "@/lib/shape";

import type {
  AccountView, CurvePoint, Figures, Fill, LiveMessage, Monkey, Order, Overview, Param, Position, Strategy, Totals,
} from "./types";

/** The result of parsing one body: a value, or what was wrong with it. */
export type Parsed<T> = { readonly status: "ok"; readonly value: T } | { readonly status: "invalid"; readonly message: string };

const SIDES = ["buy", "sell"] as const;

/** Runs `read` over an untrusted body, turning a shape failure into a value. */
export function parseWith<T>(read: (body: unknown) => T, body: unknown): Parsed<T> {
  try {
    return { status: "ok", value: read(body) };
  } catch (error) {
    if (error instanceof ShapeError) {
      return { status: "invalid", message: error.message };
    }
    throw error;
  }
}

function positionOf(fields: Fields): Position {
  return {
    market: count(fields, "market"),
    symbol: nullableText(fields, "symbol"),
    qty: decimal(fields, "qty"),
    entry: decimal(fields, "entry"),
    mark: nullableDecimal(fields, "mark"),
    unrealisedPnl: nullableDecimal(fields, "unrealised_pnl"),
    closeout: nullableDecimal(fields, "closeout"),
  };
}

function orderOf(fields: Fields): Order {
  const kind = object(fields["kind"], "kind");
  return {
    orderId: count(fields, "order_id"),
    market: count(fields, "market"),
    side: choice(fields, "side", SIDES),
    qty: decimal(fields, "qty"),
    kind:
      choice(kind, "kind", ["limit", "market"]) === "limit"
        ? { type: "limit", price: decimal(kind, "price") }
        : { type: "market", quoted: decimal(kind, "quoted") },
    reduceOnly: flag(fields, "reduce_only"),
  };
}

function monkeyOf(fields: Fields): Monkey {
  return {
    entitled: baseUnits(fields, "entitled"),
    newestEntitled: baseUnits(fields, "newest_entitled"),
    claimed: nullableBaseUnits(fields, "claimed"),
  };
}

function figuresOf(fields: Fields): Figures {
  return {
    balance: decimal(fields, "balance"),
    equity: decimal(fields, "equity"),
    margin: decimal(fields, "margin"),
    marginLevelBps: nullableInteger(fields, "margin_level_bps"),
    unrealisedPnl: decimal(fields, "unrealised_pnl"),
    realisedPnl: decimal(fields, "realised_pnl"),
    positions: items(fields, "positions", positionOf),
    orders: items(fields, "orders", orderOf),
    monkey: monkeyOf(object(fields["monkey"], "monkey")),
    timeMs: count(fields, "time_ms"),
  };
}

/** Reads a parameter's value or bound: a whole number, or a decimal string as the config writes one. */
function paramValue(value: unknown, name: string): { readonly kind: "integer" | "decimal"; readonly text: string } {
  if (typeof value === "number" && Number.isSafeInteger(value)) {
    return { kind: "integer", text: String(value) };
  }
  if (typeof value === "string" && value.length <= MAX_TEXT && isDecimal(value)) {
    return { kind: "decimal", text: value };
  }
  throw new ShapeError(`${name} is not a whole number or a decimal string`);
}

/** Reads a `[low, high]` pair of parameter values. */
function boundsOf(value: unknown, name: string): readonly [string, string] {
  const pair = list(value, `${name} bounds`);
  const [low, high] = pair;
  if (pair.length !== 2) {
    throw new ShapeError(`${name} bounds is not a pair`);
  }
  return [paramValue(low, name).text, paramValue(high, name).text];
}

/**
 * Pairs each parameter the strategy runs with the bounds the console may set it within. A `kind` entry in the
 * parameters names the strategy, as the config's does, and is not a parameter; a parameter missing from the bounds
 * is not the console's to change.
 */
function paramsOf(fields: Fields): readonly Param[] {
  const params = object(fields["params"], "params");
  const bounds = fields["bounds"] === null ? null : object(fields["bounds"], "bounds");
  return Object.keys(params)
    .filter((name) => name !== "kind")
    .map((name) => {
      const { kind, text: value } = paramValue(params[name], name);
      const limits = bounds?.[name];
      return { name, kind, value, bounds: limits === undefined ? null : boundsOf(limits, name) };
    });
}

function strategyOf(fields: Fields): Strategy {
  return {
    id: count(fields, "id"),
    kind: text(fields, "kind"),
    market: text(fields, "market"),
    bar: text(fields, "bar"),
    running: flag(fields, "running"),
    params: paramsOf(fields),
    warmedUp: flag(fields, "warmed_up"),
    target: nullableText(fields, "target"),
  };
}

function pointOf(fields: Fields): CurvePoint {
  return { timeMs: count(fields, "time_ms"), equity: decimal(fields, "equity") };
}

function accountOf(fields: Fields): AccountView {
  return {
    address: text(fields, "address"),
    state: choice(fields, "state", ["starting", "running", "stopped", "flattening", "halted"]),
    halt: nullableText(fields, "halt"),
    strategies: items(fields, "strategies", strategyOf),
    figures: fields["figures"] === null ? null : figuresOf(object(fields["figures"], "figures")),
    error: nullableText(fields, "error"),
    curve: items(fields, "curve", pointOf),
  };
}

function fillOf(fields: Fields): Fill {
  return {
    cursor: text(fields, "cursor"),
    time: count(fields, "ts"),
    market: count(fields, "market"),
    symbol: nullableText(fields, "symbol"),
    side: choice(fields, "side", SIDES),
    qty: decimal(fields, "qty"),
    price: decimal(fields, "price"),
    realisedPnl: decimal(fields, "realised_pnl"),
    profitFee: decimal(fields, "profit_fee"),
  };
}

function totalsOf(fields: Fields): Totals {
  return {
    accounts: count(fields, "accounts"),
    reporting: count(fields, "reporting"),
    balance: decimal(fields, "balance"),
    equity: decimal(fields, "equity"),
    margin: decimal(fields, "margin"),
    marginLevelBps: nullableInteger(fields, "margin_level_bps"),
    unrealisedPnl: decimal(fields, "unrealised_pnl"),
    realisedPnl: decimal(fields, "realised_pnl"),
    monkey: monkeyOf(object(fields["monkey"], "monkey")),
  };
}

function overviewOf(fields: Fields): Overview {
  return { accounts: items(fields, "accounts", accountOf), totals: totalsOf(object(fields["totals"], "totals")) };
}

/** Parses `GET /api/accounts`. */
export function readOverview(body: unknown): Overview {
  return overviewOf(object(body, "accounts body"));
}

/** Parses `GET /api/accounts/{address}`. */
export function readAccount(body: unknown): AccountView {
  return accountOf(object(body, "account body"));
}

/** Parses `GET /api/accounts/{address}/fills`. */
export function readFills(body: unknown): readonly Fill[] {
  return items(object(body, "fills body"), "fills", fillOf);
}

/** Parses one feed message. */
export function readLiveMessage(body: unknown): LiveMessage {
  const fields = object(body, "feed message");
  switch (choice(fields, "type", ["snapshot", "account", "fills", "totals"])) {
    case "snapshot":
      return { type: "snapshot", overview: overviewOf(fields) };
    case "account":
      return { type: "account", account: accountOf(object(fields["account"], "account")) };
    case "fills":
      return { type: "fills", address: text(fields, "address"), fills: items(fields, "fills", fillOf) };
    case "totals":
      return { type: "totals", totals: totalsOf(object(fields["totals"], "totals")) };
  }
}

/** Parses an error body, `{ "error": text }`; any other body reads as no message. */
export function readErrorMessage(body: unknown): string | null {
  const parsed = parseWith((value) => text(object(value, "error body"), "error"), body);
  return parsed.status === "ok" ? parsed.value : null;
}
