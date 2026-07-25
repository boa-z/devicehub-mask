import { describe, expect, it } from "vitest";
import { LatestRequestOwner } from "./latestRequest";

describe("LatestRequestOwner", () => {
  it("aborts the previous request and rejects its state commit", () => {
    const owner = new LatestRequestOwner();
    const first = owner.begin();
    const second = owner.begin();

    expect(first.signal.aborted).toBe(true);
    expect(first.isCurrent()).toBe(false);
    expect(second.signal.aborted).toBe(false);
    expect(second.isCurrent()).toBe(true);
  });

  it("invalidates the active request when its scope is cancelled", () => {
    const owner = new LatestRequestOwner();
    const ticket = owner.begin();

    owner.cancel();

    expect(ticket.signal.aborted).toBe(true);
    expect(ticket.isCurrent()).toBe(false);
  });
});
