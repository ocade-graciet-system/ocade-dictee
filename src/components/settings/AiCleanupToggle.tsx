import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface AiCleanupToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * Nettoyage du texte dicté par l'IA locale (ponctuation, accords, hésitations),
 * activé par défaut. Le garde-fou côté Rust (`summary::cleanup`) refuse toute
 * reformulation : au moindre doute, le texte dicté est collé tel quel.
 */
export const AiCleanupToggle: React.FC<AiCleanupToggleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("ai_cleanup_enabled") ?? true;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(enabled) => updateSetting("ai_cleanup_enabled", enabled)}
        isUpdating={isUpdating("ai_cleanup_enabled")}
        label={t("settings.general.aiCleanup.label")}
        description={t("settings.general.aiCleanup.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
