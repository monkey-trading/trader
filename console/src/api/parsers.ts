/** Parsers that turn the bot's JSON bodies and feed messages into typed values. */
import {
  choice, count, decimal, flag, items, nullableDecimal, nullableText, object, ShapeError, text, type Fields,
} from "@/lib/shape";

import type {
  AccountDetail, AccountList, AccountRead, AccountSummary, CurvePoint, Fill, LiveMessage, Order, Param, Position,
  Strategy,
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

/** Reads an account's figures; fields it does not know are dropped. */
function summaryOf(fields: Fields): AccountSummary {
  return {
    address: text(fields, "address"),
    name: text(fields, "name"),
    state: choice(fields, "state", ["running", "stopped", "halted"]),
    stateReason: nullableText(fields, "stateReason"),
    balance: decimal(fields, "balance"),
    equity: decimal(fields, "equity"),
    realisedPnl: decimal(fields, "realisedPnl"),
    unrealisedPnl: decimal(fields, "unrealisedPnl"),
    marginLevel: nullableDecimal(fields, "marginLevel"),
    openPositions: count(fields, "openPositions"),
    monkeyEarned: decimal(fields, "monkeyEarned"),
    monkeyClaimed: decimal(fields, "monkeyClaimed"),
  };
}

function positionOf(fields: Fields): Position {
  return {
    market: text(fields, "market"),
    qty: decimal(fields, "qty"),
    entry: decimal(fields, "entry"),
    mark: nullableDecimal(fields, "mark"),
    unrealisedPnl: decimal(fields, "unrealisedPnl"),
    closeout: nullableDecimal(fields, "closeout"),
  };
}

function orderOf(fields: Fields): Order {
  return {
    orderId: text(fields, "orderId"),
    market: text(fields, "market"),
    side: choice(fields, "side", SIDES),
    qty: decimal(fields, "qty"),
    price: nullableDecimal(fields, "price"),
    reduceOnly: flag(fields, "reduceOnly"),
  };
}

function fillOf(fields: Fields): Fill {
  return {
    id: text(fields, "id"),
    time: count(fields, "time"),
    market: text(fields, "market"),
    side: choice(fields, "side", SIDES),
    qty: decimal(fields, "qty"),
    price: decimal(fields, "price"),
    fee: decimal(fields, "fee"),
    realisedPnl: decimal(fields, "realisedPnl"),
  };
}

function pointOf(fields: Fields): CurvePoint {
  return { time: count(fields, "time"), equity: decimal(fields, "equity") };
}

function paramOf(fields: Fields): Param {
  return {
    name: text(fields, "name"),
    kind: choice(fields, "kind", ["integer", "decimal"]),
    value: decimal(fields, "value"),
    min: decimal(fields, "min"),
    max: decimal(fields, "max"),
  };
}

function strategyOf(fields: Fields): Strategy {
  return {
    id: text(fields, "id"),
    kind: text(fields, "kind"),
    market: text(fields, "market"),
    bar: text(fields, "bar"),
    state: choice(fields, "state", ["running", "stopped", "failed"]),
    reason: nullableText(fields, "reason"),
    params: items(fields, "params", paramOf),
  };
}

/** The part of an account the feed's `account` message carries. */
function liveAccountOf(fields: Fields): Omit<AccountDetail, "fills" | "curve"> {
  return {
    ...summaryOf(fields),
    positions: items(fields, "positions", positionOf),
    orders: items(fields, "orders", orderOf),
    strategies: items(fields, "strategies", strategyOf),
  };
}

function detailOf(fields: Fields): AccountDetail {
  return { ...liveAccountOf(fields), fills: items(fields, "fills", fillOf), curve: items(fields, "curve", pointOf) };
}

/** Parses `GET` of the accounts list. */
export function readAccountList(body: unknown): AccountList {
  const fields = object(body, "accounts body");
  return { seq: count(fields, "seq"), accounts: items(fields, "accounts", summaryOf) };
}

/** Parses `GET` of one account, which is also what a control returns. */
export function readAccount(body: unknown): AccountRead {
  const fields = object(body, "account body");
  return { seq: count(fields, "seq"), account: detailOf(object(fields["account"], "account")) };
}

/** Parses one feed message. */
export function readLiveMessage(body: unknown): LiveMessage {
  const fields = object(body, "feed message");
  const seq = count(fields, "seq");
  switch (choice(fields, "type", ["account", "fill", "curve"])) {
    case "account":
      return { type: "account", seq, account: liveAccountOf(object(fields["account"], "account")) };
    case "fill":
      return { type: "fill", seq, address: text(fields, "address"), fill: fillOf(object(fields["fill"], "fill")) };
    case "curve":
      return { type: "curve", seq, address: text(fields, "address"), point: pointOf(object(fields["point"], "point")) };
  }
}

/** Parses an error body, `{ "error": text }`; any other body reads as no message. */
export function readErrorMessage(body: unknown): string | null {
  const parsed = parseWith((value) => text(object(value, "error body"), "error"), body);
  return parsed.status === "ok" ? parsed.value : null;
}
