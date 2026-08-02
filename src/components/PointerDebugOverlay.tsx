import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TouchContact } from "../control";
import { displayToNativePoint, diffPointerDebugContacts, pointerDebugContactKey, type PointerDebugContact, type PointerDebugEvent } from "../pointerDebug";
import type { Orientation } from "../types";
import type { KeymapContact } from "../useDeviceVideoStream";

type Props = {
  visible: boolean;
  frameSize: { width: number; height: number };
  orientation: Orientation;
  directTouches: readonly TouchContact[];
  keymapConfigured: boolean;
  keymapContacts?: readonly KeymapContact[];
  activeMappingIds: readonly string[];
};

const maxTrailEvents = 96;

export function PointerDebugOverlay({
  visible,
  frameSize,
  orientation,
  directTouches,
  keymapConfigured,
  keymapContacts,
  activeMappingIds,
}: Props) {
  const { t } = useTranslation();
  const [renderVersion, setRenderVersion] = useState(0);
  const previousRef = useRef(new Map<string, PointerDebugContact>());
  const trailRef = useRef<PointerDebugEvent[]>([]);
  const animationFrameRef = useRef<number | null>(null);
  const contacts = useMemo<PointerDebugContact[]>(() => {
    if (keymapConfigured && keymapContacts !== undefined) {
      return keymapContacts
        .filter((contact) => contact.touching)
        .map((contact) => ({ ...contact, source: "keymap" }));
    }
    return directTouches.map((contact) => ({ ...contact, source: "direct" }));
  }, [directTouches, keymapConfigured, keymapContacts]);

  useEffect(() => {
    const scheduleRender = () => {
      if (animationFrameRef.current !== null) return;
      animationFrameRef.current = window.requestAnimationFrame(() => {
        animationFrameRef.current = null;
        setRenderVersion((version) => version + 1);
      });
    };
    if (!visible) {
      previousRef.current.clear();
      trailRef.current = [];
      scheduleRender();
      return;
    }
    const events = diffPointerDebugContacts(previousRef.current, contacts, performance.now());
    previousRef.current.clear();
    for (const contact of contacts) previousRef.current.set(pointerDebugContactKey(contact), contact);
    if (events.length) {
      trailRef.current = [...trailRef.current, ...events].slice(-maxTrailEvents);
      scheduleRender();
    }
  }, [contacts, visible]);

  useEffect(() => () => {
    if (animationFrameRef.current !== null) window.cancelAnimationFrame(animationFrameRef.current);
  }, []);

  if (!visible) return null;
  void renderVersion;
  const trail = trailRef.current;
  const latest = trail[trail.length - 1];
  return <div className="pointer-debug-overlay" aria-label={t("device.pointerDebug")}>
    <div className="pointer-debug-summary">
      <strong>{t("device.pointerDebug")}</strong>
      <span>{t("device.pointerDebugContacts", { count: contacts.length })}</span>
      {activeMappingIds.length > 0 && <span>{t("device.pointerDebugMappings", { value: activeMappingIds.join(", ") })}</span>}
      {latest && <span>{t(`device.pointerDebugAction.${latest.action}`)} · {latest.source === "direct" ? t("device.pointerDebugSource.direct") : t("device.pointerDebugSource.keymap")} {latest.identity}</span>}
    </div>
    {trail.map((event, index) => {
      const point = displayToNativePoint(event.x, event.y, orientation, frameSize);
      return <span
        key={`${event.source}:${event.identity}:${event.at}:${index}`}
        className={`pointer-debug-trail ${event.source} ${event.action}`}
        style={{ left: `${event.x * 100}%`, top: `${event.y * 100}%`, opacity: Math.max(0.08, (index + 1) / trail.length * 0.42) }}
        title={t("device.pointerDebugCoordinate", {
          displayX: `${point.displayPixelX}, ${point.displayPixelY}`,
          nativeX: `${point.nativePixelX}, ${point.nativePixelY}`,
        })}
      />;
    })}
    {contacts.map((contact) => {
      const point = displayToNativePoint(contact.x, contact.y, orientation, frameSize);
      const labelOnLeft = point.displayX > 0.68;
      return <span
        key={pointerDebugContactKey(contact)}
        className={`pointer-debug-contact ${contact.source}${labelOnLeft ? " label-left" : ""}`}
        style={{ left: `${point.displayX * 100}%`, top: `${point.displayY * 100}%` }}
      >
        <span className="pointer-debug-label">
          <strong>{contact.source === "direct" ? "D" : "K"}{contact.identity}</strong>
          <small>{t(`device.pointerDebugSource.${contact.source}`)}</small>
          <small>{t("device.pointerDebugCoordinate", {
            displayX: `${point.displayPixelX}, ${point.displayPixelY}`,
            nativeX: `${point.nativePixelX}, ${point.nativePixelY}`,
          })}</small>
        </span>
      </span>;
    })}
  </div>;
}
