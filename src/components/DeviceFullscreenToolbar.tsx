import AimOutlined from "@ant-design/icons/es/icons/AimOutlined";
import CompressOutlined from "@ant-design/icons/es/icons/CompressOutlined";
import DisconnectOutlined from "@ant-design/icons/es/icons/DisconnectOutlined";
import EyeInvisibleOutlined from "@ant-design/icons/es/icons/EyeInvisibleOutlined";
import EyeOutlined from "@ant-design/icons/es/icons/EyeOutlined";
import HolderOutlined from "@ant-design/icons/es/icons/HolderOutlined";
import LinkOutlined from "@ant-design/icons/es/icons/LinkOutlined";
import RotateLeftOutlined from "@ant-design/icons/es/icons/RotateLeftOutlined";
import RotateRightOutlined from "@ant-design/icons/es/icons/RotateRightOutlined";
import SyncOutlined from "@ant-design/icons/es/icons/SyncOutlined";
import { Button, Segmented, Tooltip } from "antd";
import { useLayoutEffect, useRef, useState, type CSSProperties, type FocusEvent, type PointerEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  attachedFullscreenToolbarPositions,
  clampToolbarPosition,
  nearestFullscreenToolbarDock,
  reconcileFullscreenToolbarDocks,
  resolveFullscreenToolbarDrop,
  shouldAttachFullscreenToolbars,
  type FullscreenToolbarDock,
  type FullscreenToolbarPositions,
  type ToolbarPoint,
} from "../fullscreenToolbarLayout";
import { KeyboardIcon } from "./KeyboardIcon";
import "./DeviceFullscreenToolbar.css";

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
  hardwareDock: FullscreenToolbarDock;
  functionDock: FullscreenToolbarDock;
  toolbarsAttached: boolean;
  hardwareControls: ReactNode;
  profileSelector: ReactNode;
  displayControls: ReactNode;
  systemFullscreenControl: ReactNode;
  onReconnect: () => void;
  onControlModeChange: (mode: DeviceControlMode) => void;
  onControlOverlayChange: () => void;
  onRotateLeft: () => void;
  onRotateRight: () => void;
  onLayoutChange: (hardwareDock: FullscreenToolbarDock, functionDock: FullscreenToolbarDock, attached: boolean) => void;
  onExit: () => void;
  onPointerEnter: (event: PointerEvent<HTMLDivElement>) => void;
  onPointerLeave: (event: PointerEvent<HTMLDivElement>) => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
};

function positionedToolbarStyle(position: ToolbarPoint): CSSProperties {
  return { left: position.x, top: position.y, right: "auto", bottom: "auto", transform: "none" };
}

function isSideDock(dock: FullscreenToolbarDock): boolean {
  return dock === "left-center" || dock === "right-center";
}

function toolbarStyle(
  drag: DragState | null,
  kind: ToolbarKind,
  attached: FullscreenToolbarPositions | null,
): CSSProperties | undefined {
  if (drag?.kind === kind) return positionedToolbarStyle(drag.position);
  if (drag?.kind === "hardware" && attached && kind === "function") {
    return positionedToolbarStyle({
      x: attached.function.x + drag.position.x - attached.hardware.x,
      y: attached.function.y + drag.position.y - attached.hardware.y,
    });
  }
  return attached ? positionedToolbarStyle(attached[kind]) : undefined;
}

export function DeviceFullscreenToolbar({
  visible,
  canReconnect,
  controlMode,
  controlOverlayVisible,
  rotationControlsLocked,
  hardwareDock,
  functionDock,
  toolbarsAttached,
  hardwareControls,
  profileSelector,
  displayControls,
  systemFullscreenControl,
  onReconnect,
  onControlModeChange,
  onControlOverlayChange,
  onRotateLeft,
  onRotateRight,
  onLayoutChange,
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
  const onLayoutChangeRef = useRef(onLayoutChange);
  onLayoutChangeRef.current = onLayoutChange;
  const [drag, setDrag] = useState<DragState | null>(null);
  const [attachedPositions, setAttachedPositions] = useState<FullscreenToolbarPositions | null>(null);
  const overlayLabel = t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay");
  const hardwareVertical = isSideDock(hardwareDock);
  const functionVertical = toolbarsAttached ? hardwareVertical : isSideDock(functionDock);

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
        const container = { width: layerBounds.width, height: layerBounds.height };
        const hardwareSize = { width: hardwareBounds.width, height: hardwareBounds.height };
        const functionSize = { width: functionBounds.width, height: functionBounds.height };
        if (toolbarsAttached) {
          const positions = attachedFullscreenToolbarPositions(hardwareDock, container, hardwareSize, functionSize);
          setAttachedPositions((current) => current
            && current.hardware.x === positions.hardware.x
            && current.hardware.y === positions.hardware.y
            && current.function.x === positions.function.x
            && current.function.y === positions.function.y
            ? current
            : positions);
          return;
        }
        setAttachedPositions(null);
        const next = reconcileFullscreenToolbarDocks(
          { hardware: hardwareDock, function: functionDock },
          container,
          hardwareSize,
          functionSize,
        );
        if (next.hardware !== hardwareDock || next.function !== functionDock) {
          onLayoutChangeRef.current(next.hardware, next.function, false);
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
  }, [functionDock, hardwareDock, toolbarsAttached]);

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
    let nextDocks = { hardware: hardwareDock, function: functionDock };
    let nextAttached = toolbarsAttached;
    if (hardwareBounds && functionBounds) {
      const hardwareSize = { width: hardwareBounds.width, height: hardwareBounds.height };
      const functionSize = { width: functionBounds.width, height: functionBounds.height };
      const nearby = shouldAttachFullscreenToolbars(
        { x: hardwareBounds.left, y: hardwareBounds.top, ...hardwareSize },
        { x: functionBounds.left, y: functionBounds.top, ...functionSize },
      );
      nextAttached = current.kind === "function" ? nearby : toolbarsAttached || nearby;
      if (nextAttached) {
        nextDocks = {
          hardware: current.kind === "hardware"
            ? nearestFullscreenToolbarDock(center, containerSize, hardwareSize)
            : hardwareDock,
          function: functionDock,
        };
      } else {
        nextDocks = resolveFullscreenToolbarDrop(
          current.kind,
          center,
          nextDocks,
          containerSize,
          hardwareSize,
          functionSize,
        );
      }
    }
    dragRef.current = null;
    setDrag(null);
    onLayoutChange(nextDocks.hardware, nextDocks.function, nextAttached);
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
        data-toolbar-attached={toolbarsAttached}
        className={`device-fullscreen-toolbar device-fullscreen-hardware-toolbar dock-${hardwareDock}${hardwareVertical ? " is-vertical" : ""}${visible ? "" : " is-hidden"}${drag?.kind === "hardware" ? " is-dragging" : ""}`}
        style={toolbarStyle(drag, "hardware", attachedPositions)}
        {...sharedEvents}
      >
        {dragHandle("hardware")}
        {hardwareControls}
      </div>
      <div
        ref={functionRef}
        data-toolbar-kind="function"
        data-toolbar-dock={functionDock}
        data-toolbar-attached={toolbarsAttached}
        className={`device-fullscreen-toolbar device-fullscreen-function-toolbar dock-${functionDock}${functionVertical ? " is-vertical" : ""}${visible ? "" : " is-hidden"}${drag?.kind === "function" ? " is-dragging" : ""}`}
        style={toolbarStyle(drag, "function", attachedPositions)}
        role="toolbar"
        aria-label={t("device.deviceFullscreenControls")}
        {...sharedEvents}
      >
        <div className="device-fullscreen-toolbar-head">
          {dragHandle("function")}
          <Tooltip title={t(toolbarsAttached ? "device.detachToolbars" : "device.attachToolbars")}>
            <Button
              aria-label={t(toolbarsAttached ? "device.detachToolbars" : "device.attachToolbars")}
              type={toolbarsAttached ? "primary" : "default"}
              icon={toolbarsAttached ? <DisconnectOutlined /> : <LinkOutlined />}
              onClick={() => onLayoutChange(hardwareDock, functionDock, !toolbarsAttached)}
            />
          </Tooltip>
        </div>
        <div className="device-fullscreen-function-controls">
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
          {profileSelector}
          {displayControls}
          {systemFullscreenControl}
          <Tooltip title={t("device.exitDeviceFullscreen")}><Button aria-label={t("device.exitDeviceFullscreen")} icon={<CompressOutlined />} onClick={onExit} /></Tooltip>
        </div>
      </div>
    </div>
  );
}
