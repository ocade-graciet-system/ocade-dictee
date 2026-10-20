import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface CopyLastTranscriptionProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Copie de la dernière transcription dans le presse-papiers, après le collage
 * (`clipboard_handling`), activée par défaut : on peut la recoller ailleurs
 * avec Ctrl+V. Désactivée, le presse-papiers retrouve son contenu d'avant.
 */
export const CopyLastTranscription: React.FC<CopyLastTranscriptionProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled =
      (getSetting("clipboard_handling") ?? "copy_to_clipboard") ===
      "copy_to_clipboard";

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(on) =>
          updateSetting(
            "clipboard_handling",
            on ? "copy_to_clipboard" : "dont_modify",
          )
        }
        isUpdating={isUpdating("clipboard_handling")}
        label={t("settings.general.copyLastTranscription.label")}
        description={t("settings.general.copyLastTranscription.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
