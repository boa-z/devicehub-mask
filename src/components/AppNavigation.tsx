import { DashboardOutlined, EnvironmentOutlined, FileTextOutlined, FolderOpenOutlined, MobileOutlined, SettingOutlined } from "@ant-design/icons";
import { Menu } from "antd";
import { useTranslation } from "react-i18next";
import { KeyboardIcon } from "./KeyboardIcon";

export type AppPage = "device" | "mappings" | "afc" | "performance" | "logs" | "location" | "settings";

type Props = {
  page: AppPage;
  onChange: (page: AppPage) => void;
};

export function AppNavigation({ page, onChange }: Props) {
  const { t } = useTranslation();

  return (
    <nav className="app-navigation" aria-label={t("navigation.label")}>
      <Menu
        mode="inline"
        inlineCollapsed
        selectedKeys={[page]}
        onSelect={({ key }) => onChange(key as AppPage)}
        items={[
          { key: "device", icon: <MobileOutlined />, label: t("navigation.device") },
          { key: "mappings", icon: <KeyboardIcon />, label: t("navigation.mappings") },
          { key: "afc", icon: <FolderOpenOutlined />, label: t("navigation.afc") },
          { key: "performance", icon: <DashboardOutlined />, label: t("navigation.performance") },
          { key: "logs", icon: <FileTextOutlined />, label: t("navigation.logs") },
          { key: "location", icon: <EnvironmentOutlined />, label: t("navigation.location") },
          { key: "settings", icon: <SettingOutlined />, label: t("navigation.settings") },
        ]}
      />
    </nav>
  );
}
