/** Checks of untrusted JSON, one field at a time, shared by every parser. */
import { isDecimal } from "./decimal";

/** A JSON object whose fields have not been checked. */
export type Fields = Readonly<Record<string, unknown>>;

/** A body or message that does not have the expected shape. */
export class ShapeError extends Error {}

/** The longest text any field may hold. */
export const MAX_TEXT = 200;

/** Reads `value` as an object, naming `where` if it is not one. */
export function object(value: unknown, where: string): Fields {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ShapeError(`${where} is not an object`);
  }
  return Object.fromEntries(Object.entries(value));
}

/** Reads `value` as an array. */
export function list(value: unknown, where: string): readonly unknown[] {
  if (!Array.isArray(value)) {
    throw new ShapeError(`${where} is not a list`);
  }
  return value;
}

/** Reads a bounded text field. */
export function text(fields: Fields, key: string): string {
  const value = fields[key];
  if (typeof value !== "string" || value.length > MAX_TEXT) {
    throw new ShapeError(`${key} is not a text`);
  }
  return value;
}

/** Reads a text field that may be null. */
export function nullableText(fields: Fields, key: string): string | null {
  return fields[key] === null ? null : text(fields, key);
}

/** Reads a decimal string field. */
export function decimal(fields: Fields, key: string): string {
  const value = text(fields, key);
  if (!isDecimal(value)) {
    throw new ShapeError(`${key} is not a decimal`);
  }
  return value;
}

/** Reads a decimal string field that may be null. */
export function nullableDecimal(fields: Fields, key: string): string | null {
  return fields[key] === null ? null : decimal(fields, key);
}

/** Reads a non-negative safe integer field. */
export function count(fields: Fields, key: string): number {
  const value = fields[key];
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new ShapeError(`${key} is not a count`);
  }
  return value;
}

/** Reads a signed safe integer field. */
export function integer(fields: Fields, key: string): number {
  const value = fields[key];
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw new ShapeError(`${key} is not an integer`);
  }
  return value;
}

/** Reads a signed safe integer field that may be null. */
export function nullableInteger(fields: Fields, key: string): number | null {
  return fields[key] === null ? null : integer(fields, key);
}

/** Reads a non-negative integer base-unit amount sent as a decimal string of digits. */
export function baseUnits(fields: Fields, key: string): string {
  const value = text(fields, key);
  if (!/^\d{1,40}$/.test(value)) {
    throw new ShapeError(`${key} is not an amount in base units`);
  }
  return value;
}

/** Reads a base-unit amount that may be null. */
export function nullableBaseUnits(fields: Fields, key: string): string | null {
  return fields[key] === null ? null : baseUnits(fields, key);
}

/** Reads a boolean field. */
export function flag(fields: Fields, key: string): boolean {
  const value = fields[key];
  if (typeof value !== "boolean") {
    throw new ShapeError(`${key} is not a boolean`);
  }
  return value;
}

/** Reads a text field that must be one of `allowed`. */
export function choice<T extends string>(fields: Fields, key: string, allowed: readonly T[]): T {
  const value = text(fields, key);
  const found = allowed.find((option) => option === value);
  if (found === undefined) {
    throw new ShapeError(`${key} is not one of ${allowed.join(", ")}`);
  }
  return found;
}

/** Reads each item of a list field with `read`. */
export function items<T>(fields: Fields, key: string, read: (item: Fields) => T): readonly T[] {
  return list(fields[key], key).map((item) => read(object(item, key)));
}
