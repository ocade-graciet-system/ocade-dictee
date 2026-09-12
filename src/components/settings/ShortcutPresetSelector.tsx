import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { commands } from "@/bindings";
import type { BindingError } from "@/bindings";
import { formatShortcut } from "../../lib/utils/shortcutFormat";
import { CustomShortcutInput } from "./CustomShortcutInput";
import { ShortcutRejectionMessage } from "./ShortcutRejectionMessage";

/** Valeur de l'entrée « Personnalisé… » dans la liste. */
const CUSTOM_VALUE = "custom";

interface ShortcutPresetSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const ShortcutPresetSelector: React.FC<ShortcutPresetSelectorProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const os = useOsType();
  const { settings, updateBinding, isUpdating } = useSettings();
  const [presets, setPresets] = useState<string[]>([]);
  const [customSelected, setCustomSelected] = useState(false);
  const [rejection, setRejection] = useState<BindingError | null>(null);

  useEffect(() => {
    commands
      .getShortcutPresets()
      .then(setPresets)
      .catch(() => setPresets([]));
  }, []);

  const current = settings?.bindings?.transcribe?.current_binding ?? null;

  // Le champ de capture apparaît si l'utilisateur choisit « Personnalisé… »
  // ou si le raccourci enregistré ne fait pas partie des préréglages.
  const isPreset = current !== null && presets.includes(current);
  const showCustom =
    customSelected || (current !== null && presets.length > 0 && !isPreset);

  const options = [
    ...presets.map((preset) => ({
      value: preset,
      label: formatShortcut(preset, os),
    })),
    {
      value: CUSTOM_VALUE,
      label: t("settings.general.shortcut.custom.option"),
    },
  ];

  const handleSelect = async (value: string) => {
    setRejection(null);
    if (value === CUSTOM_VALUE) {
      setCustomSelected(true);
      return;
    }
    setCustomSelected(false);
    setRejection(await updateBinding("transcribe", value));
  };

  // La ligne « Raccourci de dictée » garde sa mise en page horizontale ; le
  // champ personnalisé et le message de refus forment une seconde ligne du
  // groupe (`SettingsGroup` sépare ses enfants directs par un filet).
  return (
    <>
      <SettingContainer
        title={t("settings.general.shortcutPreset.title")}
        description={t("settings.general.shortcutPreset.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Dropdown
          options={options}
          selectedValue={showCustom ? CUSTOM_VALUE : current}
          onSelect={handleSelect}
          disabled={presets.length === 0 || isUpdating("binding_transcribe")}
        />
      </SettingContainer>
      {(rejection || showCustom) && (
        <div className="px-4 p-2 space-y-1">
          {rejection && <ShortcutRejectionMessage error={rejection} />}
          {showCustom && <CustomShortcutInput />}
        </div>
      )}
    </>
  );
};
