import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { MARKDOWN_ALLOWED_ELEMENTS } from "@/components/whats-new/MarkdownContent";

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
 * Markdown → HTML avec la configuration de `MarkdownContent` (react-markdown,
 * GFM, mêmes éléments autorisés, `skipHtml`), pour « Copier le compte-rendu »
 * en texte enrichi : ce qui est collé est ce qui est affiché. Seule la mise en
 * forme d'écran (classes Tailwind, ouverture des liens) est laissée de côté,
 * inutile dans un traitement de texte.
 *
 * Le HTML brut écrit dans le Markdown n'est ni interprété ni recopié : il est
 * retiré, comme à l'écran.
 */
export function markdownToHtml(markdown: string): string {
  return renderToStaticMarkup(
    createElement(
      ReactMarkdown,
      {
        allowedElements: MARKDOWN_ALLOWED_ELEMENTS,
        remarkPlugins: [remarkGfm],
        skipHtml: true,
      },
      markdown,
    ),
  );
}

/** Octets → « 1,2 Go » (gigaoctets décimaux, une décimale, format français). */
export function formatGigabytes(bytes: number): string {
  const formatter = new Intl.NumberFormat("fr-FR", {
    maximumFractionDigits: 1,
  });
  return `${formatter.format(bytes / 1_000_000_000)} Go`;
}
