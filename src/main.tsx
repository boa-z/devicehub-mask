import "@ant-design/v5-patch-for-react-19";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppProviders } from "./AppProviders";
import { installGlobalDiagnostics } from "./diagnostics";
import { i18nReady } from "./i18n";
import "./styles.css";

async function bootstrap() {
  await i18nReady;
  installGlobalDiagnostics();

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <AppProviders />
    </StrictMode>,
  );
}

void bootstrap();
