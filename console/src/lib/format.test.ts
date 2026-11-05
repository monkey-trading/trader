/** Tests of display formatting. */
import {
  formatBpsPercent, formatDecimal, formatMonkey, formatSigned, formatTime, formatTrimmed, formatUsdc, shortAddress, UNAVAILABLE,
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

describe("formatBpsPercent", () => {
  it("reads basis points as a percentage with two places, exactly", () => {
    expect(formatBpsPercent(202_500)).toBe("2,025.00%");
    expect(formatBpsPercent(5_000)).toBe("50.00%");
    expect(formatBpsPercent(7)).toBe("0.07%");
    expect(formatBpsPercent(-150)).toBe("-1.50%");
  });

  it("shows a value that is not an integer as unavailable", () => {
    expect(formatBpsPercent(1.5)).toBe(UNAVAILABLE);
  });
});

describe("formatMonkey", () => {
  it("moves the point eighteen places and truncates to two", () => {
    expect(formatMonkey("1500000000000000000")).toBe("1.50");
    expect(formatMonkey("12999999999999999999")).toBe("12.99");
    expect(formatMonkey("0")).toBe("0.00");
    expect(formatMonkey("123456789012345678901234")).toBe("123,456.78");
  });

  it("keeps every digit of an amount a float would lose", () => {
    expect(formatMonkey("9007199254740993000000000000")).toBe("9,007,199,254.74");
  });

  it("shows text that is not base units as unavailable", () => {
    expect(formatMonkey("1.5")).toBe(UNAVAILABLE);
    expect(formatMonkey("")).toBe(UNAVAILABLE);
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
