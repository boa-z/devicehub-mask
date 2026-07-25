import { useEffect, useRef } from "react";

export type RequestTicket = {
  signal: AbortSignal;
  isCurrent: () => boolean;
};

/** Owns one replaceable request and prevents a superseded response from committing state. */
export class LatestRequestOwner {
  private generation = 0;
  private controller: AbortController | null = null;

  begin(): RequestTicket {
    this.cancel();
    const generation = this.generation;
    const controller = new AbortController();
    this.controller = controller;
    return {
      signal: controller.signal,
      isCurrent: () => this.generation === generation && !controller.signal.aborted,
    };
  }

  cancel(): void {
    this.generation += 1;
    this.controller?.abort();
    this.controller = null;
  }
}

export function useLatestRequestOwner(): LatestRequestOwner {
  const owner = useRef<LatestRequestOwner | null>(null);
  if (!owner.current) owner.current = new LatestRequestOwner();

  useEffect(() => {
    const current = owner.current;
    return () => current?.cancel();
  }, []);

  return owner.current;
}
