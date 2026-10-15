import { describe, expect, it } from "vitest";
import { responsesCount } from "./survey-detail";

describe("responsesCount", () => {
  it("shows the total alone when every response is shown", () => {
    expect(responsesCount(3, 3)).toBe("3");
    expect(responsesCount(0, 0)).toBe("0");
  });

  it("says how many of the total are shown when the page is partial", () => {
    expect(responsesCount(50, 120)).toBe("50 of 120");
  });
});
