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

/**
 * Borne de sécurité : au-delà, une capture que rien n'est venu refermer
 * (fenêtre masquée sans perte de focus, session verrouillée…) s'annule d'elle
 * même. Sans elle, la première combinaison frappée dans une autre application
 * deviendrait le raccourci de dictée.
 */
const CAPTURE_TIMEOUT_MS = 15_000;

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
 *
 * La capture n'est jamais laissée armée hors du champ : elle s'annule à la
 * perte de focus de la fenêtre, quand la fenêtre est masquée (le bouton rouge
 * la masque sans démonter le composant), au premier clic ailleurs, et au bout
 * de {@link CAPTURE_TIMEOUT_MS}.
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
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  // Démarrage en cours : deux clics rapprochés tombent dans le même rendu et
  // lisent donc le même `isRecording`.
  const startingRef = useRef(false);
  // Vraie de la demande de capture jusqu'à sa fermeture, démarrage compris :
  // les gardes globales n'ont ainsi rien à annuler en dehors d'une capture.
  const armedRef = useRef(false);
  // Numéro de la capture demandée : après l'`await` de démarrage, il dit si
  // elle est toujours d'actualité (annulation entre-temps).
  const sessionRef = useRef(0);
  // Faux dès le démontage : plus aucun `setState` après cet instant.
  const mountedRef = useRef(true);

  const commit = useCallback(
    async (rawCombination: string) => {
      const combination = normalizeCapturedShortcut(rawCombination);
      const error = await updateBinding(SHORTCUT_ID, combination);
      if (!mountedRef.current) return;
      setRejection(error);
      setSaved(error === null);
    },
    [updateBinding],
  );

  const clearSafetyTimer = useCallback(() => {
    if (timeoutRef.current !== null) {
      clearTimeout(timeoutRef.current);
      timeoutRef.current = null;
    }
  }, []);

  const detachListener = useCallback(() => {
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }
  }, []);

  /**
   * Referme la capture côté technique : minuteur effacé, écouteur détaché,
   * boucle backend arrêtée. Idempotente (`armedRef`), donc appelable depuis le
   * démontage, la fin de capture et l'annulation sans double appel.
   */
  const stopCapture = useCallback(() => {
    if (!armedRef.current) return;
    armedRef.current = false;
    // Invalide un démarrage encore en vol : il refermera la boucle backend.
    sessionRef.current += 1;
    clearSafetyTimer();
    detachListener();
    commands.stopHandyKeysRecording().catch(console.error);
  }, [clearSafetyTimer, detachListener]);

  /**
   * Annule la capture et remet le champ au repos (raccourci courant affiché).
   * Aucun changement de raccourci n'est envoyé.
   */
  const cancelRecording = useCallback(() => {
    if (!armedRef.current) return;
    stopCapture();
    capturedRef.current = "";
    if (!mountedRef.current) return;
    setCaptured("");
    setIsRecording(false);
  }, [stopCapture]);

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
            cancelRecording();
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
            // Referme minuteur, écouteur et boucle backend avant l'envoi.
            stopCapture();
            setCaptured("");
            setIsRecording(false);
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

    timeoutRef.current = setTimeout(() => {
      timeoutRef.current = null;
      cancelRecording();
    }, CAPTURE_TIMEOUT_MS);

    return () => {
      cancelled = true;
      clearSafetyTimer();
      detachListener();
      // Toujours arrêter la boucle de capture côté backend, y compris au
      // démontage du composant.
      stopCapture();
    };
  }, [
    isRecording,
    commit,
    cancelRecording,
    stopCapture,
    clearSafetyTimer,
    detachListener,
  ]);

  // Gardes globales : la capture ne doit jamais survivre à la sortie du champ.
  // La fenêtre masquée par le bouton rouge ne démonte pas le composant, d'où
  // `visibilitychange` en plus de `blur`.
  useEffect(() => {
    const handlePointerDown = (event: PointerEvent) => {
      const container = containerRef.current;
      if (container && !container.contains(event.target as Node)) {
        cancelRecording();
      }
    };
    const handleVisibilityChange = () => {
      if (document.hidden) cancelRecording();
    };
    const handleBlur = () => cancelRecording();

    window.addEventListener("blur", handleBlur);
    window.addEventListener("pointerdown", handlePointerDown, true);
    document.addEventListener("visibilitychange", handleVisibilityChange);

    return () => {
      window.removeEventListener("blur", handleBlur);
      window.removeEventListener("pointerdown", handlePointerDown, true);
      document.removeEventListener("visibilitychange", handleVisibilityChange);
    };
  }, [cancelRecording]);

  // Démontage : plus aucun `setState`, et la boucle backend est refermée même
  // si le composant disparaît pendant une capture.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      clearSafetyTimer();
      detachListener();
      stopCapture();
    };
  }, [clearSafetyTimer, detachListener, stopCapture]);

  const startRecording = async () => {
    // `isRecording` ne sépare pas deux clics du même tick : sans la ref, le
    // second démarrage reçoit « Already recording », affiché à tort comme
    // « capture indisponible ».
    if (isRecording || busy || startingRef.current) return;
    startingRef.current = true;
    armedRef.current = true;
    const session = ++sessionRef.current;

    setRejection(null);
    setSaved(false);
    setUnavailable(null);
    setCaptured("");
    capturedRef.current = "";

    try {
      const result = await commands.startHandyKeysRecording(SHORTCUT_ID);
      if (result.status === "error") {
        // Backend clavier `tauri` (défaut Linux) : la capture n'existe pas.
        armedRef.current = false;
        if (mountedRef.current && sessionRef.current === session) {
          setUnavailable(result.error);
        }
        return;
      }
    } catch (error) {
      armedRef.current = false;
      if (mountedRef.current && sessionRef.current === session) {
        setUnavailable(String(error));
      }
      return;
    } finally {
      startingRef.current = false;
    }

    // Démontage ou annulation pendant l'attente : la boucle backend vient de
    // s'ouvrir, on la referme sans jamais passer en état « en capture ».
    if (!mountedRef.current || sessionRef.current !== session) {
      armedRef.current = false;
      commands.stopHandyKeysRecording().catch(console.error);
      return;
    }

    setIsRecording(true);
  };

  const fieldLabel = isRecording
    ? captured
      ? formatShortcut(captured, os)
      : t("settings.general.shortcut.custom.press")
    : formatShortcut(current, os);

  return (
    <div ref={containerRef} className="mt-2 space-y-1">
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
