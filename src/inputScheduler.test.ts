import { describe, expect, it, vi } from "vitest";
import { createDemandScheduler } from "./inputScheduler";

describe("demand input scheduler", () => {
  it("does not allocate an interval while idle", () => {
    const schedule = vi.fn(() => 1);
    const scheduler = createDemandScheduler(schedule, vi.fn(), vi.fn());

    expect(schedule).not.toHaveBeenCalled();
    expect(scheduler.isRunning()).toBe(false);
  });

  it("starts once on demand and stops the active interval", () => {
    const schedule = vi.fn(() => 7);
    const cancel = vi.fn();
    const tick = vi.fn();
    const scheduler = createDemandScheduler(schedule, cancel, tick);

    scheduler.start();
    scheduler.start();
    expect(schedule).toHaveBeenCalledTimes(1);
    expect(schedule).toHaveBeenCalledWith(tick, 16);
    expect(scheduler.isRunning()).toBe(true);

    scheduler.stop();
    scheduler.stop();
    expect(cancel).toHaveBeenCalledOnce();
    expect(cancel).toHaveBeenCalledWith(7);
    expect(scheduler.isRunning()).toBe(false);
  });

  it("cleans up and cannot restart after disposal", () => {
    const schedule = vi.fn(() => 3);
    const cancel = vi.fn();
    const scheduler = createDemandScheduler(schedule, cancel, vi.fn());

    scheduler.start();
    scheduler.dispose();
    scheduler.start();

    expect(cancel).toHaveBeenCalledWith(3);
    expect(schedule).toHaveBeenCalledTimes(1);
  });
});
