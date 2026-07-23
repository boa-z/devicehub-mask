export const maxPasteTextCharacters = 1024;

export function truncatePasteText(text: string): string {
  return Array.from(text).slice(0, maxPasteTextCharacters).join("");
}
