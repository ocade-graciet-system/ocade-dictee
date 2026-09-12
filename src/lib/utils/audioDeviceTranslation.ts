import type { TFunction } from "i18next";
import type { AudioDevice } from "@/bindings";

/**
 * Get the display label for an audio device: the translated "Default" label
 * for the backend's sentinel entry (is_default, or name "Default"/"default"
 * for callers that predate the is_default field), otherwise its raw name.
 * @param device - The audio device returned by the backend
 * @param t - The translation function from useTranslation
 * @returns The label to show in the microphone/output device dropdowns
 */
export function getAudioDeviceLabel(device: AudioDevice, t: TFunction): string {
  const isDefault =
    device.is_default || device.name === "Default" || device.name === "default";
  return isDefault ? t("settings.sound.defaultDevice") : device.name;
}
