import React from "react";
import { useTranslation } from "react-i18next";
import type { BindingError } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { formatShortcut } from "../../lib/utils/shortcutFormat";

interface ShortcutRejectionMessageProps {
  error: BindingError;
}

/**
 * Message affiché sous le champ : raison du refus + raccourci conservé.
 * Les clés i18n sont écrites littéralement pour rester vérifiables par
 * `bun run check:translations`.
 */
export const ShortcutRejectionMessage: React.FC<
  ShortcutRejectionMessageProps
> = ({ error }) => {
  const { t } = useTranslation();
  const os = useOsType();
  const { settings } = useSettings();

  // `previousBinding` est vide quand le backend n'a pas pu désigner de
  // raccourci conservé (`unknownBinding`, échec de l'appel). Le raccourci
  // resté en place est alors celui du réglage, que le store a restauré.
  const previous =
    error.previousBinding ||
    settings?.bindings?.transcribe?.current_binding ||
    "";

  // Défini dans le composant : `t` garde ainsi son type i18next d'origine.
  const reason = (): string => {
    switch (error.code) {
      case "empty":
        return t("settings.general.shortcut.rejections.empty");
      case "unparseable":
        return t("settings.general.shortcut.rejections.unparseable");
      case "noKey":
        return t("settings.general.shortcut.rejections.noKey");
      case "multipleKeys":
        return t("settings.general.shortcut.rejections.multipleKeys");
      case "noModifier":
        return t("settings.general.shortcut.rejections.noModifier");
      case "shiftOnlyWithPrintable":
        return t("settings.general.shortcut.rejections.shiftOnlyWithPrintable");
      case "escapeKey":
        return t("settings.general.shortcut.rejections.escapeKey");
      case "reservedBySystem":
        return t("settings.general.shortcut.rejections.reservedBySystem", {
          combo: formatShortcut(error.detail ?? "", os),
        });
      case "registrationFailed":
        // Sans complément du système, ne pas afficher de parenthèses vides.
        return error.detail
          ? t("settings.general.shortcut.rejections.registrationFailed", {
              detail: error.detail,
            })
          : t(
              "settings.general.shortcut.rejections.registrationFailedNoDetail",
            );
      case "unknownBinding":
        return t("settings.general.shortcut.rejections.unknownBinding");
    }
  };

  return (
    <p className="text-xs text-red-500" role="alert">
      {t("settings.general.shortcut.custom.rejected", {
        reason: reason(),
        previous: formatShortcut(previous, os),
      })}
    </p>
  );
};
