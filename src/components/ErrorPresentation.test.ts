import { describe, expect, it } from "vitest";
import { errorText } from "../errorPresentation";

describe("error presentation", () => {
  it("copies the useful message from Error values", () => {
    expect(errorText(new Error("connection closed"))).toBe("connection closed");
  });

  it("preserves backend error details", () => {
    const detail = "NSCocoaErrorDomain code 4865: Expected to find key includeContainerPaths.";
    expect(errorText(detail)).toBe(detail);
  });
});
