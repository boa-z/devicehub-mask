import { EnvironmentOutlined } from "@ant-design/icons";
import { Button, InputNumber, Space, Tag, Typography, message } from "antd";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { validLocationCoordinates } from "../location";
import type { LocationStatus } from "../types";
import { ErrorAlert } from "./ErrorPresentation";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  activeUdid: string | null;
  status: LocationStatus;
  request: Request;
};

type Preset = { key: "cupertino" | "taipei" | "tokyo"; latitude: number; longitude: number };

const presets: Preset[] = [
  { key: "cupertino", latitude: 37.33182, longitude: -122.03118 },
  { key: "taipei", latitude: 25.033, longitude: 121.5654 },
  { key: "tokyo", latitude: 35.6812, longitude: 139.7671 },
];

async function requireSuccess(response: Response) {
  if (!response.ok) throw new Error((await response.text()) || `${response.status} ${response.statusText}`);
}

export function LocationPage({ activeUdid, status, request }: Props) {
  const { t } = useTranslation();
  const [latitude, setLatitude] = useState<number | null>(status.latitude ?? 25.033);
  const [longitude, setLongitude] = useState<number | null>(status.longitude ?? 121.5654);
  const [operation, setOperation] = useState<"set" | "clear" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [toast, contextHolder] = message.useMessage();

  useEffect(() => {
    if (status.latitude !== null) setLatitude(status.latitude);
    if (status.longitude !== null) setLongitude(status.longitude);
  }, [status.latitude, status.longitude]);

  const available = Boolean(activeUdid) && status.available;
  const apply = async () => {
    if (!validLocationCoordinates(latitude, longitude)) {
      setError(t("location.invalidCoordinates"));
      return;
    }
    setOperation("set");
    setError(null);
    try {
      await requireSuccess(await request("/api/device/location", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ latitude, longitude }),
      }));
      void toast.success(t("location.applied"));
    } catch (requestError) {
      setError(String(requestError));
    } finally {
      setOperation(null);
    }
  };

  const clear = async () => {
    setOperation("clear");
    setError(null);
    try {
      await requireSuccess(await request("/api/device/location", { method: "DELETE" }));
      void toast.success(t("location.cleared"));
    } catch (requestError) {
      setError(String(requestError));
    } finally {
      setOperation(null);
    }
  };

  const stateTag = !activeUdid
    ? <Tag>{t("location.disconnected")}</Tag>
    : status.active
      ? <Tag color="success">{t("location.active")}</Tag>
      : status.available
        ? <Tag color="processing">{t("location.ready")}</Tag>
        : <Tag color="warning">{t("location.unavailable")}</Tag>;

  return (
    <main className="location-page">
      {contextHolder}
      <header>
        <div>
          <Typography.Title level={3}><EnvironmentOutlined /> {t("location.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("location.subtitle")}</Typography.Text>
        </div>
        <Space>
          {status.backend && <Tag>{t(`location.backends.${status.backend}`)}</Tag>}
          {stateTag}
        </Space>
      </header>

      {(error || status.error) && <ErrorAlert title={t("common.error")} error={error ?? status.error} />}

      <section className="location-section" aria-labelledby="location-coordinates-title">
        <Typography.Title id="location-coordinates-title" level={5}>{t("location.coordinates")}</Typography.Title>
        <div className="location-fields">
          <label>
            <span>{t("location.latitude")}</span>
            <InputNumber
              aria-label={t("location.latitude")}
              value={latitude}
              min={-90}
              max={90}
              precision={8}
              step={0.000001}
              onChange={setLatitude}
            />
          </label>
          <label>
            <span>{t("location.longitude")}</span>
            <InputNumber
              aria-label={t("location.longitude")}
              value={longitude}
              min={-180}
              max={180}
              precision={8}
              step={0.000001}
              onChange={setLongitude}
            />
          </label>
        </div>
        <Space wrap>
          <Button
            type="primary"
            icon={<EnvironmentOutlined />}
            disabled={!available || !validLocationCoordinates(latitude, longitude)}
            loading={operation === "set"}
            onClick={() => void apply()}
          >
            {t("location.apply")}
          </Button>
          {status.active && (
            <Button danger loading={operation === "clear"} onClick={() => void clear()}>
              {t("location.stop")}
            </Button>
          )}
        </Space>
      </section>

      <section className="location-section" aria-labelledby="location-presets-title">
        <Typography.Title id="location-presets-title" level={5}>{t("location.presets")}</Typography.Title>
        <div className="location-presets">
          {presets.map((preset) => (
            <button
              type="button"
              key={preset.key}
              onClick={() => {
                setLatitude(preset.latitude);
                setLongitude(preset.longitude);
                setError(null);
              }}
            >
              <strong>{t(`location.presetNames.${preset.key}`)}</strong>
              <span>{preset.latitude.toFixed(5)}, {preset.longitude.toFixed(5)}</span>
            </button>
          ))}
        </div>
      </section>
    </main>
  );
}
