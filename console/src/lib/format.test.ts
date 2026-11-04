/** Tests of display formatting. */
import {
  formatDecimal, formatRatioPercent, formatSigned, formatTime, formatTrimmed, formatUsdc, shortAddress, UNAVAILABLE,
} from "./format";

describe("formatUsdc", () => {
  it("groups thousands and keeps two places, truncating", () => {
    expect(formatUsdc("1234567.899")).toBe("1,234,567.89");
    expect(formatUsdc("-1000")).toBe("-1,000.00");
    expect(formatUsdc("0")).toBe("0.00");
  });

  it("keeps every digit of a value a float would round", () => {
    expect(formatUsdc("9007199254740993.12")).toBe("9,007,199,254,740,993.12");
  });

  it("shows a value it cannot read as unavailable, never as zero", () => {
    expect(formatUsdc("abc")).toBe(UNAVAILABLE);
    expect(formatDecimal("", 2)).toBe(UNAVAILABLE);
  });
});

describe("formatSigned", () => {
  it("marks gains with a plus, losses with a minus, and leaves zero bare", () => {
    expect([formatSigned("12.5"), formatSigned("-12.5"), formatSigned("0"), formatSigned("0.004")]).toEqual([
      "+12.50", "-12.50", "0.00", "0.00",
    ]);
  });
});

describe("formatTrimmed", () => {
  it("drops trailing zeros and keeps integers whole", () => {
    expect(formatTrimmed("0.100000", 6)).toBe("0.1");
    expect(formatTrimmed("60000", 4)).toBe("60,000");
    expect(formatTrimmed("-2.5000", 4)).toBe("-2.5");
  });
});

describe("formatRatioPercent", () => {
  it("shifts the decimal point two places", () => {
    expect(formatRatioPercent("2.5")).toBe("250.00%");
    expect(formatRatioPercent("33.114754098")).toBe("3,311.47%");
  });
});

describe("shortAddress and formatTime", () => {
  it("shortens a long address and leaves a short one", () => {
    expect(shortAddress("0x1111111111111111111111111111111111111111")).toBe("0x1111…1111");
    expect(shortAddress("0xabc")).toBe("0xabc");
  });

  it("renders UTC regardless of the machine", () => {
    expect(formatTime(1_700_000_000_000)).toBe("2023-11-14 22:13:20");
  });
});
