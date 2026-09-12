import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import type { BindingError } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { useOsType } from "../../hooks/useOsType";
import {
  formatShortcut,
  normalizeCapturedShortcut,
} from "../../lib/utils/shortcutFormat";
import { ShortcutRejectionMessage } from "./ShortcutRejectionMessage";

const SHORTCUT_ID = "transcribe";

/** Charge utile de l'événement `handy-keys-event` (Rust : `FrontendKeyEvent`). */
interface HandyKeysEvent {
  modifiers: string[];
  key: string | null;
  is_key_down: boolean;
  hotkey_string: string;
}

/**
 * Capture d'un raccourci personnalisé (enregistreur handy-keys d'origine,
 * traduit et simplifié). Le raccourci courant reste actif tant qu'aucune
 * capture n'a été acceptée par le backend ; un refus fait revenir le champ au
 * raccourci précédent et affiche la raison.
 */
export const CustomShortcutInput: React.FC = () => {
  const { t } = useTranslation();
  const os = useOsType();
  const { settings, updateBinding, isUpdating } = useSettings();

  const current = settings?.bindings?.transcribe?.current_binding ?? "";
  const busy = isUpdating(`binding_${SHORTCUT_ID}`);

  const [isRecording, setIsRecording] = useState(false);
  const [captured, setCaptured] = useState("");
  const [rejection, setRejection] = useState<BindingError | null>(null);
  const [saved, setSaved] = useState(false);
  const [unavailable, setUnavailable] = useState<string | null>(null);

  // Ref : le gestionnaire d'événement lit la dernière capture sans dépendre
  // d'une fermeture périmée.
  const capturedRef = useRef("");
  const unlistenRef = useRef<(() => void) | null>(null);
  // Démarrage en cours : deux clics rapprochés tombent dans le même rendu et
  // lisent donc le même `isRecording`.
  const startingRef = useRef(false);

  const commit = useCallback(
    async (rawCombination: string) => {
      const combination = normalizeCapturedShortcut(rawCombination);
      const error = await updateBinding(SHORTCUT_ID, combination);
      setRejection(error);
      setSaved(error === null);
    },
    [updateBinding],
  );

  useEffect(() => {
    if (!isRecording) return;

    let cancelled = false;

    const setup = async () => {
      const unlisten = await listen<HandyKeysEvent>(
        "handy-keys-event",
        async (event) => {
          if (cancelled) return;
          const { hotkey_string, key, is_key_down } = event.payload;

          // Échap annule la capture sans rien changer.
          if (is_key_down && key === "escape") {
            capturedRef.current = "";
            setCaptured("");
            setIsRecording(false);
            return;
          }

          if (is_key_down && hotkey_string) {
            // Le champ suit la frappe en direct, modificateurs seuls compris.
            setCaptured(hotkey_string);
            // Mais seule une combinaison portant une touche est mémorisée :
            // un modificateur seul émet `key: null`, et son relâchement (y
            // compris un relâchement synthétique sous Windows/Linux) ne doit
            // pas valider une combinaison sans touche.
            if (key !== null) {
              capturedRef.current = hotkey_string;
            }
            return;
          }

          // Relâchement : la capture ne se termine que si une combinaison
          // complète a été mémorisée.
          if (!is_key_down && capturedRef.current) {
            const combination = capturedRef.current;
            capturedRef.current = "";
            setCaptured("");
            setIsRecording(false); // déclenche le nettoyage ci-dessous
            await commit(combination);
          }
        },
      );

      if (cancelled) {
        unlisten();
        return;
      }
      unlistenRef.current = unlisten;
    };

    setup();

    return () => {
      cancelled = true;
      if (unlistenRef.current) {
        unlistenRef.current();
        unlistenRef.current = null;
      }
      // Toujours arrêter la boucle de capture côté backend, y compris au
      // démontage du composant.
      commands.stopHandyKeysRecording().catch(console.error);
    };
  }, [isRecording, commit]);

  const startRecording = async () => {
    // `isRecording` ne sépare pas deux clics du même tick : sans la ref, le
    // second démarrage reçoit « Already recording », affiché à tort comme
    // « capture indisponible ».
    if (isRecording || busy || startingRef.current) return;
    startingRef.current = true;

    setRejection(null);
    setSaved(false);
    setUnavailable(null);
    setCaptured("");
    capturedRef.current = "";

    try {
      const result = await commands.startHandyKeysRecording(SHORTCUT_ID);
      if (result.status === "error") {
        // Backend clavier `tauri` (défaut Linux) : la capture n'existe pas.
        setUnavailable(result.error);
        return;
      }
    } catch (error) {
      setUnavailable(String(error));
      return;
    } finally {
      startingRef.current = false;
    }
    setIsRecording(true);
  };

  const fieldLabel = isRecording
    ? captured
      ? formatShortcut(captured, os)
      : t("settings.general.shortcut.custom.press")
    : formatShortcut(current, os);

  return (
    <div className="mt-2 space-y-1">
      <button
        type="button"
        onClick={startRecording}
        disabled={busy}
        aria-label={t("settings.general.shortcut.custom.label")}
        className={`px-2 py-1 text-sm font-semibold rounded-md border ${
          isRecording
            ? "border-logo-primary bg-logo-primary/30"
            : "bg-mid-gray/10 border-mid-gray/80 hover:bg-logo-primary/10 hover:border-logo-primary cursor-pointer"
        } ${busy ? "opacity-50" : ""}`}
      >
        {fieldLabel}
      </button>

      <p className="text-xs text-mid-gray">
        {t("settings.general.shortcut.custom.description")}
      </p>

      {rejection && <ShortcutRejectionMessage error={rejection} />}

      {saved && !rejection && (
        <p className="text-xs text-mid-gray">
          {t("settings.general.shortcut.custom.saved")}
        </p>
      )}

      {unavailable !== null && (
        <p className="text-xs text-red-500" role="alert">
          {t("settings.general.shortcut.custom.unavailable", {
            error: unavailable,
          })}
        </p>
      )}
    </div>
  );
};
