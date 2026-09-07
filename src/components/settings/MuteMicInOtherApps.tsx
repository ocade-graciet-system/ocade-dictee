import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { SettingContainer } from "../ui/SettingContainer";
import { Input } from "../ui/Input";
import { Dropdown, type DropdownOption } from "../ui/Dropdown";
import { useSettings } from "../../hooks/useSettings";
import type { CommMuteMode } from "@/bindings";

interface MuteMicInOtherAppsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const MuteMicInOtherApps: React.FC<MuteMicInOtherAppsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("mute_others_while_recording") ?? false;
    const shortcut = getSetting("comm_mute_shortcut") ?? "ctrl+alt+shift+m";
    const mode = (getSetting("comm_mute_mode") ??
      "push_to_mute") as CommMuteMode;

    const description = t("settings.sound.muteMicInOtherApps.description");

    const modeOptions: DropdownOption[] = [
      {
        value: "push_to_mute",
        label: t("settings.sound.muteMicInOtherApps.modePushToMute"),
      },
      {
        value: "toggle",
        label: t("settings.sound.muteMicInOtherApps.modeToggle"),
      },
    ];

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) =>
            updateSetting("mute_others_while_recording", value)
          }
          isUpdating={isUpdating("mute_others_while_recording")}
          label={t("settings.sound.muteMicInOtherApps.title")}
          description={description}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {enabled && (
          <>
            <SettingContainer
              title={t("settings.sound.muteMicInOtherApps.shortcutLabel")}
              description={description}
              descriptionMode={descriptionMode}
              grouped={grouped}
            >
              <Input
                type="text"
                value={shortcut}
                onChange={(e) =>
                  updateSetting("comm_mute_shortcut", e.target.value)
                }
                placeholder={t(
                  "settings.sound.muteMicInOtherApps.shortcutPlaceholder",
                )}
                disabled={isUpdating("comm_mute_shortcut")}
              />
            </SettingContainer>
            <SettingContainer
              title={t("settings.sound.muteMicInOtherApps.modeLabel")}
              description={description}
              descriptionMode={descriptionMode}
              grouped={grouped}
            >
              <Dropdown
                options={modeOptions}
                selectedValue={mode}
                onSelect={(value) =>
                  updateSetting("comm_mute_mode", value as CommMuteMode)
                }
                disabled={isUpdating("comm_mute_mode")}
              />
            </SettingContainer>
          </>
        )}
      </>
    );
  },
);

MuteMicInOtherApps.displayName = "MuteMicInOtherApps";
