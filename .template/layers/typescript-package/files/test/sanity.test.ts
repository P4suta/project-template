import { describe, it, expect } from "vitest";
import { ready } from "../src/index.ts";

describe("scaffold", () => {
  it("reports ready", () => {
    expect(ready()).toBe(true);
  });
});
