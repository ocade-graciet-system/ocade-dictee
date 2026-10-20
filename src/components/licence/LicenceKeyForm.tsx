import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleAlert, Loader2 } from "lucide-react";
import type { LicenceError } from "@/bindings";
import {
  formatKeyInput,
  isCompleteKey,
  licenceErrorMessage,
  normalizeKey,
} from "./format";

interface LicenceKeyFormProps {
  /** Envoie la clé ; renvoie l'erreur à afficher, ou `null` en cas de succès. */
  onSubmit: (key: string) => Promise<LicenceError | null>;
  submitLabel: string;
  busy: boolean;
  /** Bouton pleine largeur et champ centré (écran de blocage). */
  centered?: boolean;
  autoFocus?: boolean;
}

/**
 * Saisie d'une clé de licence : mise en forme au fil de la frappe, bouton
 * actif seulement quand la clé est complète, erreur affichée sous le champ.
 */
export const LicenceKeyForm: React.FC<LicenceKeyFormProps> = ({
  onSubmit,
  submitLabel,
  busy,
  centered = false,
  autoFocus = true,
}) => {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (autoFocus) inputRef.current?.focus();
  }, [autoFocus]);

  // Validation locale, même règle que le serveur : une faute de frappe est
  // signalée tout de suite, sans appel réseau (le serveur limite les essais).
  const complete = isCompleteKey(value);
  const normalized = complete ? normalizeKey(value) : null;
  const localError =
    complete && normalized === null
      ? t("licence.errors.invalid_key_format")
      : null;
  const shownError = error ?? localError;

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (normalized === null || busy) return;
    setError(null);
    const err = await onSubmit(normalized);
    if (err) setError(licenceErrorMessage(t, err));
  };

  return (
    <form onSubmit={(e) => void submit(e)} className="w-full space-y-3">
      <label className={`block ${centered ? "text-center" : ""}`}>
        <span className="sr-only">{t("licence.input.label")}</span>
        <input
          ref={inputRef}
          value={value}
          onChange={(e) => {
            setValue(formatKeyInput(e.target.value));
            setError(null);
          }}
          placeholder={t("licence.input.placeholder")}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={shownError !== null}
          className={`w-full rounded-lg border border-mid-gray/40 bg-mid-gray/10 px-3 py-2.5 font-mono text-base tracking-wider transition-colors placeholder:text-text/30 hover:border-logo-primary focus:border-logo-primary focus:bg-logo-primary/10 focus:outline-none ${centered ? "text-center" : ""}`}
        />
      </label>
      <p
        className={`text-xs text-text/50 text-pretty ${centered ? "text-center" : ""}`}
      >
        {t("licence.input.help")}
      </p>

      {shownError && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2.5 text-left text-xs text-red-400"
        >
          <CircleAlert className="mt-px h-3.5 w-3.5 shrink-0" />
          <span className="text-pretty">{shownError}</span>
        </div>
      )}

      <button
        type="submit"
        disabled={normalized === null || busy}
        className={`inline-flex items-center justify-center gap-2 rounded-lg bg-logo-primary px-5 py-2.5 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-logo-primary/85 disabled:cursor-default disabled:opacity-50 ${centered ? "w-full" : ""}`}
      >
        {busy && (
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
        )}
        {submitLabel}
      </button>
    </form>
  );
};
