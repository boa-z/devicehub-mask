import { message } from "antd";
import { createElement } from "react";
import { ErrorCopyButton } from "./components/ErrorPresentation";
import { errorText } from "./errorPresentation";

type ErrorMessageOptions = {
  key?: string | number;
};

export function showErrorMessage(error: unknown, options: ErrorMessageOptions = {}) {
  const detail = errorText(error);
  return message.error({
    ...options,
    duration: 8,
    content: createElement(
      "span",
      { className: "user-error-message" },
      createElement("span", null, detail),
      createElement(ErrorCopyButton, { className: "user-error-message-copy", error: detail }),
    ),
  });
}
