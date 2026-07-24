import { FolderOpenOutlined } from "@ant-design/icons";
import { Empty, Tag, Typography } from "antd";
import { useTranslation } from "react-i18next";
import { DeviceFilesPane } from "./DeviceFilesPane";

type Request = (path: string, init?: RequestInit) => Promise<Response>;

type Props = {
  active: boolean;
  activeUdid: string | null;
  request: Request;
};

export function AfcPage({ active, activeUdid, request }: Props) {
  const { t } = useTranslation();

  return (
    <main className="afc-page" hidden={!active}>
      <header>
        <div>
          <Typography.Title level={3}><FolderOpenOutlined /> {t("afc.title")}</Typography.Title>
          <Typography.Text type="secondary">{t("afc.subtitle")}</Typography.Text>
        </div>
        <Tag color={activeUdid ? "success" : "default"}>
          {t(activeUdid ? "afc.connected" : "afc.disconnected")}
        </Tag>
      </header>

      {activeUdid ? (
        <section className="afc-browser" aria-label={t("afc.title")}>
          <DeviceFilesPane
            active={active}
            deviceId={activeUdid}
            refreshToken={0}
            request={request}
          />
        </section>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t("afc.connectDevice")} />
      )}
    </main>
  );
}
