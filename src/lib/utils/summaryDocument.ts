import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/** Séparateur entre la transcription et le compte-rendu dans le `.md` exporté. */
export const EXPORT_SEPARATOR = "\n\n---\n\n";

/**
 * Document produit par « Enregistrer sous… » : la transcription, puis une
 * ligne `---`, puis le compte-rendu s'il existe.
 */
export function buildExportMarkdown(
  transcription: string,
  summary: string | null,
): string {
  const body = transcription.trimEnd();
  if (!summary || !summary.trim()) return `${body}\n`;
  return `${body}${EXPORT_SEPARATOR}${summary.trim()}\n`;
}

/**
 * Markdown → HTML avec la bibliothèque déjà utilisée par `MarkdownContent`
 * (react-markdown + GFM), pour « Copier le compte-rendu » en texte enrichi.
 * Le HTML brut du Markdown est échappé, jamais interprété.
 */
export function markdownToHtml(markdown: string): string {
  return renderToStaticMarkup(
    createElement(ReactMarkdown, { remarkPlugins: [remarkGfm] }, markdown),
  );
}

/** Octets → « 1,2 Go » (gigaoctets décimaux, une décimale, format français). */
export function formatGigabytes(bytes: number): string {
  const formatter = new Intl.NumberFormat("fr-FR", {
    maximumFractionDigits: 1,
  });
  return `${formatter.format(bytes / 1_000_000_000)} Go`;
}
