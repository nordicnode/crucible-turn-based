import { describe, expect, it } from "vitest";
import { formatUptime } from "./format";

describe("formatUptime", () => {
  it("formats sub-minute durations", () => {
    expect(formatUptime(0)).toBe("0s");
    expect(formatUptime(59)).toBe("59s");
  });

  it("formats minutes and hours", () => {
    expect(formatUptime(61)).toBe("1m 1s");
    expect(formatUptime(3661)).toBe("1h 1m");
  });

  it("formats days", () => {
    expect(formatUptime(90061)).toBe("1d 1h");
  });

  it("clamps negatives", () => {
    expect(formatUptime(-5)).toBe("0s");
  });
});
