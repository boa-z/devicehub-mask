import {
  AimOutlined,
  CompressOutlined,
  EyeInvisibleOutlined,
  EyeOutlined,
  HolderOutlined,
  MoreOutlined,
  RotateLeftOutlined,
  RotateRightOutlined,
  SyncOutlined,
} from "@ant-design/icons";
import { Button, Popover, Segmented, Tooltip } from "antd";
import { useLayoutEffect, useRef, useState, type CSSProperties, type FocusEvent, type PointerEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  clampToolbarPosition,
  reconcileFullscreenToolbarDocks,
  resolveFullscreenToolbarDrop,
  type FullscreenToolbarDock,
  type ToolbarPoint,
} from "../fullscreenToolbarLayout";
import { KeyboardIcon } from "./KeyboardIcon";

export type DeviceControlMode = "mapping" | "keyboard";
type ToolbarKind = "hardware" | "function";
type DragState = {
  kind: ToolbarKind;
  pointerId: number;
  offsetX: number;
  offsetY: number;
  position: ToolbarPoint;
};

type Props = {
  visible: boolean;
  canReconnect: boolean;
  controlMode: DeviceControlMode;
  controlOverlayVisible: boolean;
  rotationControlsLocked: boolean;
  overflowOpen: boolean;
  hardwareDock: FullscreenToolbarDock;
  functionDock: FullscreenToolbarDock;
  hardwareControls: ReactNode;
  profileSelector: ReactNode;
  displayControls: ReactNode;
  systemFullscreenControl: ReactNode;
  onReconnect: () => void;
  onControlModeChange: (mode: DeviceControlMode) => void;
  onControlOverlayChange: () => void;
  onRotateLeft: () => void;
  onRotateRight: () => void;
  onOverflowOpenChange: (open: boolean) => void;
  onDocksChange: (hardwareDock: FullscreenToolbarDock, functionDock: FullscreenToolbarDock) => void;
  onExit: () => void;
  onPointerEnter: (event: PointerEvent<HTMLDivElement>) => void;
  onPointerLeave: (event: PointerEvent<HTMLDivElement>) => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
};

function toolbarStyle(drag: DragState | null, kind: ToolbarKind): CSSProperties | undefined {
  if (!drag || drag.kind !== kind) return undefined;
  return { left: drag.position.x, top: drag.position.y, right: "auto", bottom: "auto", transform: "none" };
}

export function DeviceFullscreenToolbar({
  visible,
  canReconnect,
  controlMode,
  controlOverlayVisible,
  rotationControlsLocked,
  overflowOpen,
  hardwareDock,
  functionDock,
  hardwareControls,
  profileSelector,
  displayControls,
  systemFullscreenControl,
  onReconnect,
  onControlModeChange,
  onControlOverlayChange,
  onRotateLeft,
  onRotateRight,
  onOverflowOpenChange,
  onDocksChange,
  onExit,
  onPointerEnter,
  onPointerLeave,
  onFocus,
  onBlur,
}: Props) {
  const { t } = useTranslation();
  const layerRef = useRef<HTMLDivElement>(null);
  const hardwareRef = useRef<HTMLDivElement>(null);
  const functionRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<DragState | null>(null);
  const onDocksChangeRef = useRef(onDocksChange);
  onDocksChangeRef.current = onDocksChange;
  const [drag, setDrag] = useState<DragState | null>(null);
  const overlayLabel = t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay");

  useLayoutEffect(() => {
    const layer = layerRef.current;
    const hardware = hardwareRef.current;
    const functions = functionRef.current;
    if (!layer || !hardware || !functions || typeof ResizeObserver === "undefined") return;

    let frame = 0;
    const reconcile = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        const layerBounds = layer.getBoundingClientRect();
        const hardwareBounds = hardware.getBoundingClientRect();
        const functionBounds = functions.getBoundingClientRect();
        const next = reconcileFullscreenToolbarDocks(
          { hardware: hardwareDock, function: functionDock },
          { width: layerBounds.width, height: layerBounds.height },
          { width: hardwareBounds.width, height: hardwareBounds.height },
          { width: functionBounds.width, height: functionBounds.height },
        );
        if (next.hardware !== hardwareDock || next.function !== functionDock) {
          onDocksChangeRef.current(next.hardware, next.function);
        }
      });
    };
    const observer = new ResizeObserver(reconcile);
    observer.observe(layer);
    observer.observe(hardware);
    observer.observe(functions);
    reconcile();
    return () => {
      observer.disconnect();
      window.cancelAnimationFrame(frame);
    };
  }, [functionDock, hardwareDock]);

  const toolbarRef = (kind: ToolbarKind) => kind === "hardware" ? hardwareRef : functionRef;
  const startDrag = (kind: ToolbarKind, event: PointerEvent<HTMLElement>) => {
    const layer = layerRef.current;
    const toolbar = toolbarRef(kind).current;
    if (!layer || !toolbar) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    const layerBounds = layer.getBoundingClientRect();
    const toolbarBounds = toolbar.getBoundingClientRect();
    const next: DragState = {
      kind,
      pointerId: event.pointerId,
      offsetX: event.clientX - toolbarBounds.left,
      offsetY: event.clientY - toolbarBounds.top,
      position: { x: toolbarBounds.left - layerBounds.left, y: toolbarBounds.top - layerBounds.top },
    };
    dragRef.current = next;
    setDrag(next);
  };
  const moveDrag = (event: PointerEvent<HTMLElement>) => {
    const current = dragRef.current;
    const layer = layerRef.current;
    const toolbar = current ? toolbarRef(current.kind).current : null;
    if (!current || current.pointerId !== event.pointerId || !layer || !toolbar) return;
    event.preventDefault();
    const layerBounds = layer.getBoundingClientRect();
    const toolbarBounds = toolbar.getBoundingClientRect();
    const position = clampToolbarPosition(
      { x: event.clientX - layerBounds.left - current.offsetX, y: event.clientY - layerBounds.top - current.offsetY },
      { width: layerBounds.width, height: layerBounds.height },
      { width: toolbarBounds.width, height: toolbarBounds.height },
    );
    const next = { ...current, position };
    dragRef.current = next;
    setDrag(next);
  };
  const finishDrag = (event: PointerEvent<HTMLElement>) => {
    const current = dragRef.current;
    const layer = layerRef.current;
    const toolbar = current ? toolbarRef(current.kind).current : null;
    if (!current || current.pointerId !== event.pointerId || !layer || !toolbar) return;
    event.preventDefault();
    const layerBounds = layer.getBoundingClientRect();
    const toolbarBounds = toolbar.getBoundingClientRect();
    const containerSize = { width: layerBounds.width, height: layerBounds.height };
    const toolbarSize = { width: toolbarBounds.width, height: toolbarBounds.height };
    const center = {
      x: current.position.x + toolbarSize.width / 2,
      y: current.position.y + toolbarSize.height / 2,
    };
    const hardwareBounds = hardwareRef.current?.getBoundingClientRect();
    const functionBounds = functionRef.current?.getBoundingClientRect();
    const nextDocks = hardwareBounds && functionBounds
      ? resolveFullscreenToolbarDrop(
        current.kind,
        center,
        { hardware: hardwareDock, function: functionDock },
        containerSize,
        { width: hardwareBounds.width, height: hardwareBounds.height },
        { width: functionBounds.width, height: functionBounds.height },
      )
      : { hardware: hardwareDock, function: functionDock };
    dragRef.current = null;
    setDrag(null);
    onDocksChange(nextDocks.hardware, nextDocks.function);
  };

  const dragHandle = (kind: ToolbarKind) => {
    const label = t(kind === "hardware" ? "device.moveHardwareToolbar" : "device.moveFunctionToolbar");
    return (
      <Tooltip title={label}>
        <Button
          className="device-fullscreen-drag-handle"
          aria-label={label}
          icon={<HolderOutlined />}
          onPointerDown={(event) => startDrag(kind, event)}
          onPointerMove={moveDrag}
          onPointerUp={finishDrag}
          onPointerCancel={finishDrag}
        />
      </Tooltip>
    );
  };

  const sharedEvents = { onPointerEnter, onPointerLeave, onFocusCapture: onFocus, onBlurCapture: onBlur };
  return (
    <div className="device-fullscreen-toolbars" ref={layerRef}>
      <div
        ref={hardwareRef}
        data-toolbar-kind="hardware"
        data-toolbar-dock={hardwareDock}
        className={`device-fullscreen-toolbar device-fullscreen-hardware-toolbar dock-${hardwareDock}${visible ? "" : " is-hidden"}${drag?.kind === "hardware" ? " is-dragging" : ""}`}
        style={toolbarStyle(drag, "hardware")}
        {...sharedEvents}
      >
        {dragHandle("hardware")}
        {hardwareControls}
      </div>
      <div
        ref={functionRef}
        data-toolbar-kind="function"
        data-toolbar-dock={functionDock}
        className={`device-fullscreen-toolbar device-fullscreen-function-toolbar dock-${functionDock}${visible ? "" : " is-hidden"}${drag?.kind === "function" ? " is-dragging" : ""}`}
        style={toolbarStyle(drag, "function")}
        role="toolbar"
        aria-label={t("device.deviceFullscreenControls")}
        {...sharedEvents}
      >
        {dragHandle("function")}
        <Tooltip title={t("device.reconnect")}>
          <Button aria-label={t("device.reconnect")} disabled={!canReconnect} icon={<SyncOutlined />} onClick={onReconnect} />
        </Tooltip>
        <Segmented<DeviceControlMode>
          value={controlMode}
          options={[
            { label: <Tooltip title={t("device.mappingMode")}><AimOutlined /></Tooltip>, value: "mapping" },
            { label: <Tooltip title={t("device.keyboardMode")}><KeyboardIcon /></Tooltip>, value: "keyboard" },
          ]}
          onChange={onControlModeChange}
        />
        <Tooltip title={overlayLabel}>
          <Button aria-label={overlayLabel} icon={controlOverlayVisible ? <EyeInvisibleOutlined /> : <EyeOutlined />} onClick={onControlOverlayChange} />
        </Tooltip>
        <Tooltip title={t("device.rotateLeft")}><Button aria-label={t("device.rotateLeft")} disabled={rotationControlsLocked} icon={<RotateLeftOutlined />} onClick={onRotateLeft} /></Tooltip>
        <Tooltip title={t("device.rotateRight")}><Button aria-label={t("device.rotateRight")} disabled={rotationControlsLocked} icon={<RotateRightOutlined />} onClick={onRotateRight} /></Tooltip>
        <Popover
          trigger="click"
          open={overflowOpen}
          onOpenChange={onOverflowOpenChange}
          content={(
            <div className="device-fullscreen-overflow">
              <div className="device-fullscreen-overflow-row">{profileSelector}</div>
              <div className="device-fullscreen-overflow-row">{displayControls}</div>
              <div className="device-fullscreen-overflow-row is-window-control">{systemFullscreenControl}</div>
            </div>
          )}
        >
          <Tooltip title={t("device.moreControls")}><Button aria-label={t("device.moreControls")} type={overflowOpen ? "primary" : "default"} icon={<MoreOutlined />} /></Tooltip>
        </Popover>
        <Tooltip title={t("device.exitDeviceFullscreen")}><Button aria-label={t("device.exitDeviceFullscreen")} icon={<CompressOutlined />} onClick={onExit} /></Tooltip>
      </div>
    </div>
  );
}
