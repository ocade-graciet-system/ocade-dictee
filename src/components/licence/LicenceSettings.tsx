import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { KeyRound, Loader2, RefreshCw, Trash2 } from "lucide-react";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Dialog } from "../ui/Dialog";
import { useLicenceStore } from "@/stores/licenceStore";
import { LicenceKeyForm } from "./LicenceKeyForm";
import {
  daysLabel,
  formatDate,
  formatDateTime,
  licenceErrorMessage,
  stateTone,
  type Tone,
} from "./format";

const TONE_DOT: Record<Tone, string> = {
  ok: "bg-green-500",
  warning: "bg-amber-400",
  error: "bg-red-500",
  neutral: "bg-mid-gray/60",
};

/** Ligne d'information, alignée sur les lignes des autres onglets. */
const Row: React.FC<{ label: string; children: React.ReactNode }> = ({
  label,
  children,
}) => (
  <div className="flex min-h-12 items-center justify-between gap-4 px-4 py-2">
    <h3 className="text-sm font-medium">{label}</h3>
    <div className="text-right text-sm">{children}</div>
  </div>
);

/**
 * Onglet Licence : état de la licence sur cet ordinateur, échéance et jours
 * restants, saisie, changement et retrait de la clé, vérification immédiate.
 */
export const LicenceSettings: React.FC = () => {
  const { t } = useTranslation();
  const { view, busy, initialize, activate, change, refresh, remove } =
    useLicenceStore();
  const [dialog, setDialog] = useState<"enter" | "change" | "remove" | null>(
    null,
  );

  useEffect(() => {
    void initialize();
  }, [initialize]);

  if (!view) return null;

  const hasKey = view.key_suffix !== null;
  const tone = stateTone(view.state);

  const onRefresh = async () => {
    const err = await refresh();
    if (err) toast.error(licenceErrorMessage(t, err));
    else toast.success(t("licence.success.refreshed"));
  };

  const onRemove = async () => {
    const err = await remove();
    if (err) {
      toast.error(licenceErrorMessage(t, err));
    } else {
      setDialog(null);
      toast.success(t("licence.success.removed"));
    }
  };

  const submitKey = async (key: string) => {
    const err = dialog === "change" ? await change(key) : await activate(key);
    if (!err) {
      setDialog(null);
      toast.success(t("licence.success.activated"));
    }
    return err;
  };

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6">
      <SettingsGroup title={t("licence.title")}>
        <Row label={t("licence.fields.state")}>
          <span className="inline-flex items-center gap-2">
            <span
              className={`h-2.5 w-2.5 shrink-0 rounded-full ${TONE_DOT[tone]}`}
              aria-hidden="true"
            />
            {t(`licence.states.${view.state}`)}
          </span>
        </Row>

        {hasKey && (
          <Row label={t("licence.fields.key")}>
            {/* eslint-disable-next-line i18next/no-literal-string */}
            <span className="font-mono tracking-wider">
              OCAD-••••-••••-••••-{view.key_suffix}
            </span>
          </Row>
        )}

        {view.expires_at !== null ? (
          <Row label={t("licence.fields.expires")}>
            <span>{formatDate(view.expires_at)}</span>
            <span className="block text-xs text-text/60">
              {daysLabel(t, view.days_left ?? 0)}
            </span>
          </Row>
        ) : (
          hasKey &&
          view.token_expires_at !== null && (
            <Row label={t("licence.fields.expires")}>
              {t("licence.fields.noExpiry")}
            </Row>
          )
        )}

        {view.seats_max !== null && view.seats_used !== null && (
          <Row label={t("licence.fields.seats")}>
            {t("licence.fields.seatsValue", {
              used: view.seats_used,
              max: view.seats_max,
            })}
          </Row>
        )}

        {view.hostname && (
          <Row label={t("licence.fields.computer")}>{view.hostname}</Row>
        )}

        {hasKey && (
          <Row label={t("licence.fields.lastCheck")}>
            {view.last_check_at !== null
              ? formatDateTime(view.last_check_at)
              : t("licence.fields.never")}
          </Row>
        )}

        <div className="flex flex-wrap items-center justify-end gap-2 px-4 py-3">
          {hasKey ? (
            <>
              {/* Poste libéré par un opérateur : la vérification ne
                  réactive jamais seule, il faut ressaisir la clé. */}
              {view.state === "not_activated_here" && (
                <button
                  type="button"
                  disabled={busy !== null}
                  onClick={() => setDialog("enter")}
                  className="inline-flex items-center gap-2 rounded-lg bg-logo-primary px-3 py-1.5 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-logo-primary/85 disabled:cursor-default disabled:opacity-50"
                >
                  <KeyRound className="h-4 w-4" />
                  {t("licence.actions.enter")}
                </button>
              )}
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => void onRefresh()}
                className="inline-flex items-center gap-2 rounded-lg border border-mid-gray/20 bg-mid-gray/10 px-3 py-1.5 text-sm font-medium transition-colors hover:border-logo-primary disabled:cursor-default disabled:opacity-50"
              >
                {busy === "refresh" ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <RefreshCw className="h-4 w-4" />
                )}
                {t("licence.actions.refresh")}
              </button>
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => setDialog("change")}
                className="inline-flex items-center gap-2 rounded-lg border border-mid-gray/20 bg-mid-gray/10 px-3 py-1.5 text-sm font-medium transition-colors hover:border-logo-primary disabled:cursor-default disabled:opacity-50"
              >
                <KeyRound className="h-4 w-4" />
                {t("licence.actions.change")}
              </button>
              <button
                type="button"
                disabled={busy !== null}
                onClick={() => setDialog("remove")}
                className="inline-flex items-center gap-2 rounded-lg border border-transparent px-3 py-1.5 text-sm font-medium text-red-400 transition-colors hover:bg-red-500/10 disabled:cursor-default disabled:opacity-50"
              >
                <Trash2 className="h-4 w-4" />
                {t("licence.actions.remove")}
              </button>
            </>
          ) : (
            <button
              type="button"
              onClick={() => setDialog("enter")}
              className="inline-flex items-center gap-2 rounded-lg bg-logo-primary px-4 py-2 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-logo-primary/85"
            >
              <KeyRound className="h-4 w-4" />
              {t("licence.actions.enter")}
            </button>
          )}
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("licence.privacy.title")}>
        <p className="px-4 py-3 text-sm leading-relaxed text-text/70 text-pretty">
          {t("licence.privacy.body")}
        </p>
      </SettingsGroup>

      <Dialog
        open={dialog === "enter" || dialog === "change"}
        title={
          dialog === "change"
            ? t("licence.dialog.changeTitle")
            : t("licence.dialog.enterTitle")
        }
        description={
          dialog === "change" ? t("licence.dialog.changeHelp") : undefined
        }
        closeLabel={t("licence.actions.close")}
        onOpenChange={(open) => {
          if (!open && busy === null) setDialog(null);
        }}
      >
        <LicenceKeyForm
          key={dialog ?? "closed"}
          onSubmit={submitKey}
          submitLabel={
            dialog === "change"
              ? t("licence.actions.change")
              : t("licence.actions.activate")
          }
          busy={busy === "activate" || busy === "change"}
        />
      </Dialog>

      <Dialog
        open={dialog === "remove"}
        title={t("licence.dialog.removeTitle")}
        description={t("licence.dialog.removeHelp")}
        closeLabel={t("licence.actions.close")}
        onOpenChange={(open) => {
          if (!open && busy === null) setDialog(null);
        }}
        footer={
          <div className="flex justify-end gap-2">
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => setDialog(null)}
              className="rounded-lg border border-mid-gray/20 bg-mid-gray/10 px-4 py-1.5 text-sm font-medium transition-colors hover:border-logo-primary disabled:opacity-50"
            >
              {t("licence.actions.cancel")}
            </button>
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void onRemove()}
              className="inline-flex items-center gap-2 rounded-lg bg-red-600 px-4 py-1.5 text-sm font-semibold text-white transition-colors hover:bg-red-700 disabled:opacity-50"
            >
              {busy === "remove" && (
                <Loader2 className="h-4 w-4 animate-spin" />
              )}
              {t("licence.actions.remove")}
            </button>
          </div>
        }
      >
        <span />
      </Dialog>
    </div>
  );
};
