import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import { commands } from "@/bindings";

const MAC_LABELS: Record<string, string> = {
  ctrl: "⌃",
  option: "⌥",
  alt: "⌥",
  shift: "⇧",
  command: "⌘",
  space: "Espace",
};
const OTHER_LABELS: Record<string, string> = {
  ctrl: "Ctrl",
  option: "Alt",
  alt: "Alt",
  shift: "Maj",
  command: "Win",
  space: "Espace",
};

/** "ctrl+option+space" → "⌃ ⌥ Espace" (macOS) ou "Ctrl + Alt + Espace". */
export const formatPreset = (preset: string, os: string): string => {
  const table = os === "macos" ? MAC_LABELS : OTHER_LABELS;
  const parts = preset.split("+").map((p) => table[p] ?? p.toUpperCase());
  return os === "macos" ? parts.join(" ") : parts.join(" + ");
};

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

  useEffect(() => {
    commands
      .getShortcutPresets()
      .then(setPresets)
      .catch(() => setPresets([]));
  }, []);

  const current = settings?.bindings?.transcribe?.current_binding ?? null;
  const options = presets.map((preset) => ({
    value: preset,
    label: formatPreset(preset, os),
  }));

  return (
    <SettingContainer
      title={t("settings.general.shortcutPreset.title")}
      description={t("settings.general.shortcutPreset.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <Dropdown
        options={options}
        selectedValue={current}
        onSelect={(value) => updateBinding("transcribe", value)}
        disabled={presets.length === 0 || isUpdating("binding_transcribe")}
      />
    </SettingContainer>
  );
};
