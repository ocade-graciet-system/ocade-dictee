import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface AudioFeedbackToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/** Bip de début et de fin de dictée (actif par défaut, désactivable). */
export const AudioFeedbackToggle: React.FC<AudioFeedbackToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("audio_feedback") ?? true;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(enabled) => updateSetting("audio_feedback", enabled)}
        isUpdating={isUpdating("audio_feedback")}
        label={t("settings.sound.audioFeedback.label")}
        description={t("settings.sound.audioFeedback.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  });
