/** Tests of exact decimal arithmetic. */
import { compareDecimals, isDecimal, parseUnits, signOf, sumUnits, unitsToText } from "./decimal";

describe("isDecimal", () => {
  it("accepts plain decimals and refuses everything else", () => {
    expect(["0", "-1.5", "123456789.123456789", "0.000000000000000001"].map(isDecimal)).toEqual([true, true, true, true]);
    expect(["", "-", "1e3", "1.", ".5", "+1", "1,5", "NaN", "0.0000000000000000001", " 1"].map(isDecimal)).toEqual(
      Array.from({ length: 10 }, () => false),
    );
  });
});

describe("parseUnits and unitsToText", () => {
  it("round-trips a value past the safe integer range without loss", () => {
    const text = "123456789012345.123456789";
    expect(unitsToText(parseUnits(text), 9)).toBe(text);
    expect(parseUnits("9007199254740993")).toBe(9_007_199_254_740_993n * 10n ** 18n);
  });

  it("truncates toward zero rather than rounding", () => {
    expect(unitsToText(parseUnits("1.999"), 2)).toBe("1.99");
    expect(unitsToText(parseUnits("-1.999"), 2)).toBe("-1.99");
    expect(unitsToText(parseUnits("2.5"), 0)).toBe("2");
  });

  it("drops the sign of a value that truncates to zero", () => {
    expect(unitsToText(parseUnits("-0.004"), 2)).toBe("0.00");
  });

  it("throws on text that is not a decimal and on places out of range", () => {
    expect(() => parseUnits("1e3")).toThrow("not a decimal");
    expect(() => unitsToText(1n, 19)).toThrow("places out of range");
  });
});

describe("sums and comparisons", () => {
  it("sums exactly where floats would drift", () => {
    expect(sumUnits(["0.1", "0.2"])).toBe(parseUnits("0.3"));
  });

  it("gives null when any term is unreadable", () => {
    expect(sumUnits(["1", "x"])).toBeNull();
  });

  it("compares and signs by value, not by text", () => {
    expect(compareDecimals("10", "9.99")).toBe(1);
    expect(compareDecimals("1.0", "1")).toBe(0);
    expect(compareDecimals("-2", "-1")).toBe(-1);
    expect([signOf("-0.5"), signOf("0.0"), signOf("3")]).toEqual([-1, 0, 1]);
  });
});
