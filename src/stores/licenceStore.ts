import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { commands, type LicenceError, type LicenceView } from "@/bindings";

/** Relecture périodique : le temps qui passe change l'état (jours restants, échéance). */
const RELOAD_INTERVAL_MS = 60 * 60 * 1000;

type BusyAction = "activate" | "change" | "refresh" | "remove";

interface LicenceStore {
  view: LicenceView | null;
  busy: BusyAction | null;
  initialized: boolean;
  initialize: () => Promise<void>;
  load: () => Promise<void>;
  /** Chaque action renvoie l'erreur à afficher, ou `null` en cas de succès. */
  activate: (key: string) => Promise<LicenceError | null>;
  change: (key: string) => Promise<LicenceError | null>;
  refresh: () => Promise<LicenceError | null>;
  remove: () => Promise<LicenceError | null>;
}

const unexpected = (e: unknown): LicenceError => ({
  code: "generic",
  seats_used: null,
  seats_max: null,
  expired_at: null,
  detail: String(e),
});

export const useLicenceStore = create<LicenceStore>()((set, get) => {
  // Exécute une commande, met la vue à jour et rend l'erreur éventuelle.
  const run = async (
    action: BusyAction,
    call: () => ReturnType<typeof commands.licenceRefresh>,
  ): Promise<LicenceError | null> => {
    set({ busy: action });
    try {
      const result = await call();
      if (result.status === "ok") {
        set({ view: result.data });
        return null;
      }
      // L'état local a pu changer (refus définitif mémorisé) : on le relit.
      await get().load();
      return result.error;
    } catch (e) {
      return unexpected(e);
    } finally {
      set({ busy: null });
    }
  };

  return {
    view: null,
    busy: null,
    initialized: false,

    load: async () => {
      try {
        set({ view: await commands.licenceGetState() });
      } catch (e) {
        console.error("Lecture de l'état de licence impossible :", e);
      }
    },

    initialize: async () => {
      if (get().initialized) return;
      set({ initialized: true });
      await get().load();
      listen<LicenceView>("licence-state-changed", (event) => {
        set({ view: event.payload });
      });
      setInterval(() => void get().load(), RELOAD_INTERVAL_MS);
    },

    activate: (key) => run("activate", () => commands.licenceActivate(key)),
    change: (key) => run("change", () => commands.licenceChange(key)),
    refresh: () => run("refresh", () => commands.licenceRefresh()),
    remove: () => run("remove", () => commands.licenceRemove()),
  };
});
