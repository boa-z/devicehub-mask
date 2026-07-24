export function errorText(error: unknown) {
  if (error instanceof Error) return error.message || error.name;
  return String(error);
}

export async function copyErrorText(error: unknown) {
  const text = errorText(error);
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) throw new Error("Clipboard API is unavailable");
}
