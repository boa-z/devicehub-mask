import { CopyOutlined } from "@ant-design/icons";
import { Alert, Button, Tooltip, message, type AlertProps } from "antd";
import type { ReactNode } from "react";
import { copyErrorText, errorText } from "../errorPresentation";
import i18n from "../i18n";

type CopyButtonProps = {
  error: unknown;
  className?: string;
};

export function ErrorCopyButton({ error, className }: CopyButtonProps) {
  const copy = async () => {
    try {
      await copyErrorText(error);
      void message.success(i18n.t("common.errorCopied"));
    } catch {
      void message.warning(i18n.t("common.errorCopyFailed"));
    }
  };

  return (
    <Tooltip title={i18n.t("common.copyError")}>
      <Button
        className={className}
        type="text"
        size="small"
        aria-label={i18n.t("common.copyError")}
        icon={<CopyOutlined />}
        onClick={() => void copy()}
      />
    </Tooltip>
  );
}

type ErrorAlertProps = Omit<AlertProps, "action" | "description" | "message"> & {
  title: ReactNode;
  error: unknown;
};

export function ErrorAlert({ title, error, type = "error", ...props }: ErrorAlertProps) {
  const detail = errorText(error);
  return (
    <Alert
      {...props}
      type={type}
      showIcon
      message={title}
      description={detail}
      action={<ErrorCopyButton error={detail} />}
    />
  );
}
