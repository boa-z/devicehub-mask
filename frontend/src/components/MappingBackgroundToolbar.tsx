import {
  CameraOutlined,
  DownloadOutlined,
  PictureOutlined,
  VideoCameraOutlined,
} from "@ant-design/icons";
import { Button, Segmented, Space, Tag, Tooltip, Typography } from "antd";
import { useTranslation } from "react-i18next";

export type MappingBackgroundMode = "live" | "screenshot";

type Size = { width: number; height: number };

type Props = {
  mode: MappingBackgroundMode;
  sourceSize: Size;
  viewportSize: Size;
  screenshotAvailable: boolean;
  canCapture: boolean;
  onModeChange: (mode: MappingBackgroundMode) => void;
  onCapture: () => void;
  onSave: () => void;
};

export function MappingBackgroundToolbar({
  mode,
  sourceSize,
  viewportSize,
  screenshotAvailable,
  canCapture,
  onModeChange,
  onCapture,
  onSave,
}: Props) {
  const { t } = useTranslation();
  const sourceWidth = Math.round(sourceSize.width);
  const sourceHeight = Math.round(sourceSize.height);
  const displayWidth = Math.round(viewportSize.width);
  const displayHeight = Math.round(viewportSize.height);
  const scale = sourceWidth > 0 && sourceHeight > 0
    ? Math.min(displayWidth / sourceWidth, displayHeight / sourceHeight) * 100
    : 0;

  return (
    <div className="mapping-editor-toolbar">
      <div className="mapping-resolution" aria-label={t("mapping.resolutionLabel")}>
        <Typography.Text strong>{t("mapping.sourceResolution", { width: sourceWidth, height: sourceHeight })}</Typography.Text>
        <span aria-hidden="true">→</span>
        <Typography.Text type="secondary">{t("mapping.adaptedResolution", { width: displayWidth, height: displayHeight })}</Typography.Text>
        <Tag>{t("mapping.scale", { value: scale.toFixed(0) })}</Tag>
      </div>
      <Space size={6} className="mapping-background-controls">
        <Typography.Text type="secondary">{t("mapping.background")}</Typography.Text>
        <Segmented<MappingBackgroundMode>
          value={mode}
          options={[
            { value: "live", label: t("mapping.liveBackground"), icon: <VideoCameraOutlined /> },
            { value: "screenshot", label: t("mapping.screenshotBackground"), icon: <PictureOutlined />, disabled: !screenshotAvailable },
          ]}
          onChange={onModeChange}
        />
        <Tooltip title={t(screenshotAvailable ? "mapping.retakeScreenshot" : "mapping.captureScreenshot")}>
          <Button disabled={!canCapture} icon={<CameraOutlined />} onClick={onCapture} />
        </Tooltip>
        <Tooltip title={t("mapping.saveScreenshot")}>
          <Button disabled={!screenshotAvailable && !canCapture} icon={<DownloadOutlined />} onClick={onSave} />
        </Tooltip>
      </Space>
    </div>
  );
}
