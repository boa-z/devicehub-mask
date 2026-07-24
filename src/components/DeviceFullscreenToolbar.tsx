import {
  AimOutlined,
  CompressOutlined,
  EyeInvisibleOutlined,
  EyeOutlined,
  HomeOutlined,
  MoreOutlined,
  RotateLeftOutlined,
  RotateRightOutlined,
  SyncOutlined,
} from "@ant-design/icons";
import { Button, Popover, Segmented, Tooltip } from "antd";
import type { FocusEvent, PointerEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { KeyboardIcon } from "./KeyboardIcon";

export type DeviceControlMode = "mapping" | "keyboard";

type Props = {
  visible: boolean;
  canReconnect: boolean;
  controlMode: DeviceControlMode;
  controlOverlayVisible: boolean;
  rotationControlsLocked: boolean;
  overflowOpen: boolean;
  profileSelector: ReactNode;
  displayControls: ReactNode;
  secondaryHardwareControls: ReactNode;
  systemFullscreenControl: ReactNode;
  onReconnect: () => void;
  onControlModeChange: (mode: DeviceControlMode) => void;
  onControlOverlayChange: () => void;
  onHome: () => void;
  onRotateLeft: () => void;
  onRotateRight: () => void;
  onOverflowOpenChange: (open: boolean) => void;
  onExit: () => void;
  onPointerEnter: (event: PointerEvent<HTMLDivElement>) => void;
  onPointerLeave: (event: PointerEvent<HTMLDivElement>) => void;
  onFocus: (event: FocusEvent<HTMLDivElement>) => void;
  onBlur: (event: FocusEvent<HTMLDivElement>) => void;
};

export function DeviceFullscreenToolbar({
  visible,
  canReconnect,
  controlMode,
  controlOverlayVisible,
  rotationControlsLocked,
  overflowOpen,
  profileSelector,
  displayControls,
  secondaryHardwareControls,
  systemFullscreenControl,
  onReconnect,
  onControlModeChange,
  onControlOverlayChange,
  onHome,
  onRotateLeft,
  onRotateRight,
  onOverflowOpenChange,
  onExit,
  onPointerEnter,
  onPointerLeave,
  onFocus,
  onBlur,
}: Props) {
  const { t } = useTranslation();
  const overlayLabel = t(controlOverlayVisible ? "device.hideControlOverlay" : "device.showControlOverlay");

  return (
    <div
      className={`device-fullscreen-toolbar${visible ? "" : " is-hidden"}`}
      role="toolbar"
      aria-label={t("device.deviceFullscreenControls")}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
      onFocusCapture={onFocus}
      onBlurCapture={onBlur}
    >
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
        <Button
          aria-label={overlayLabel}
          icon={controlOverlayVisible ? <EyeInvisibleOutlined /> : <EyeOutlined />}
          onClick={onControlOverlayChange}
        />
      </Tooltip>
      <Tooltip title={t("hardware.home")}><Button aria-label={t("hardware.home")} icon={<HomeOutlined />} onClick={onHome} /></Tooltip>
      <Tooltip title={t("device.rotateLeft")}><Button aria-label={t("device.rotateLeft")} disabled={rotationControlsLocked} icon={<RotateLeftOutlined />} onClick={onRotateLeft} /></Tooltip>
      <Tooltip title={t("device.rotateRight")}><Button aria-label={t("device.rotateRight")} disabled={rotationControlsLocked} icon={<RotateRightOutlined />} onClick={onRotateRight} /></Tooltip>
      <Popover
        trigger="click"
        placement="bottom"
        open={overflowOpen}
        onOpenChange={onOverflowOpenChange}
        content={(
          <div className="device-fullscreen-overflow">
            <div className="device-fullscreen-overflow-row">{profileSelector}</div>
            <div className="device-fullscreen-overflow-row">{displayControls}</div>
            <div className="device-fullscreen-overflow-row">{secondaryHardwareControls}</div>
            <div className="device-fullscreen-overflow-row is-window-control">{systemFullscreenControl}</div>
          </div>
        )}
      >
        <Tooltip title={t("device.moreControls")}>
          <Button aria-label={t("device.moreControls")} type={overflowOpen ? "primary" : "default"} icon={<MoreOutlined />} />
        </Tooltip>
      </Popover>
      <Tooltip title={t("device.exitDeviceFullscreen")}><Button aria-label={t("device.exitDeviceFullscreen")} icon={<CompressOutlined />} onClick={onExit} /></Tooltip>
    </div>
  );
}
