export type DemandScheduler = {
  start: () => void;
  stop: () => void;
  isRunning: () => boolean;
  dispose: () => void;
};

/** Runs a periodic task only while input state needs time-based updates. */
export function createDemandScheduler<Handle>(
  schedule: (callback: () => void, intervalMs: number) => Handle,
  cancel: (handle: Handle) => void,
  tick: () => void,
  intervalMs = 16,
): DemandScheduler {
  let handle: Handle | undefined;
  let disposed = false;

  const stop = () => {
    if (handle === undefined) return;
    cancel(handle);
    handle = undefined;
  };

  return {
    start() {
      if (disposed || handle !== undefined) return;
      handle = schedule(tick, intervalMs);
    },
    stop,
    isRunning() {
      return handle !== undefined;
    },
    dispose() {
      disposed = true;
      stop();
    },
  };
}
