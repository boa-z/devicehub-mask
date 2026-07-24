import { Spin } from "antd";

export function WorkspaceLoading({ inspector = false }: { inspector?: boolean }) {
  return (
    <div className={`workspace-loading${inspector ? " workspace-loading-inspector" : ""}`} aria-busy="true">
      <Spin size="small" />
    </div>
  );
}
