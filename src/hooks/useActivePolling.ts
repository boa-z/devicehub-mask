import { useEffect, useRef } from "react";

export function useActivePolling(
  enabled: boolean,
  poll: () => Promise<unknown>,
  intervalMs: number,
): void {
  const pollRef = useRef(poll);
  pollRef.current = poll;

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const run = async () => {
      try {
        await pollRef.current();
      } catch {
        // The owning view surfaces actionable request failures.
      }
      if (!cancelled) timer = setTimeout(run, intervalMs);
    };

    void run();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [enabled, intervalMs]);
}
