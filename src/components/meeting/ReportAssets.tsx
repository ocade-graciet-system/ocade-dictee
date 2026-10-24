import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  BookOpenText,
  ClipboardPaste,
  FileUp,
  ImagePlus,
  Palette,
  Pencil,
  Sparkles,
  Trash2,
} from "lucide-react";
import {
  commands,
  type GraphicCharter,
  type TextAsset,
  type TextAssetKind,
} from "@/bindings";
import { useMeetingStore } from "@/stores/meetingStore";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Textarea } from "../ui/Textarea";
import { TEXT_ASSET_WORD_LIMITS } from "./format";

const TEXT_EXTENSIONS: Record<TextAssetKind, string[]> = {
  editorial: ["md", "markdown", "txt", "docx"],
  skill: ["md", "markdown", "txt", "skill", "zip"],
};
const LOGO_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "svg", "gif"];
const FONTS = [
  "Arial",
  "Calibri",
  "Cambria",
  "Georgia",
  "Helvetica",
  "Lato",
  "Montserrat",
  "Open Sans",
  "Poppins",
  "Roboto",
  "Segoe UI",
  "Times New Roman",
  "Verdana",
];
const DEFAULT_CHARTER: GraphicCharter = {
  primaryColor: "#6D3BF5",
  secondaryColor: "#1F2937",
  font: "Arial",
  footer: "",
};

const formatDate = (unixSeconds: number, language: string) =>
  new Date(unixSeconds * 1000).toLocaleDateString(language, {
    dateStyle: "short",
  });

const sectionTitle = "text-sm font-semibold text-text";
const sectionHint = "text-xs text-text/50";

interface TextAssetBlockProps {
  kind: TextAssetKind;
  asset: TextAsset | null;
  icon: React.ElementType;
  onEdit: (kind: TextAssetKind) => void;
}

// Charte éditoriale ou skill : import d'un fichier, ou texte collé et
// modifiable dans une fenêtre.
const TextAssetBlock: React.FC<TextAssetBlockProps> = ({
  kind,
  asset,
  icon: Icon,
  onEdit,
}) => {
  const { t, i18n } = useTranslation();
  const applyAssets = useMeetingStore((s) => s.applyAssets);
  const [importing, setImporting] = useState(false);

  const handleImport = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: t(`settings.meeting.assets.${kind}.fileType`),
          extensions: TEXT_EXTENSIONS[kind],
        },
      ],
    });
    if (typeof selected !== "string") return;
    setImporting(true);
    try {
      applyAssets(await commands.meetingImportTextAsset(kind, selected));
    } finally {
      setImporting(false);
    }
  };

  const handleClear = async () => {
    const confirmed = await ask(
      t(`settings.meeting.assets.${kind}.clearConfirm`),
      { title: t(`settings.meeting.assets.${kind}.title`), kind: "warning" },
    );
    if (confirmed) applyAssets(await commands.meetingClearTextAsset(kind));
  };

  const wordLimit = TEXT_ASSET_WORD_LIMITS[kind];
  const tooLong = asset !== null && asset.words > wordLimit;

  return (
    <div className="space-y-2 p-4">
      <div className="flex items-start gap-3">
        <Icon className="mt-0.5 h-5 w-5 shrink-0 text-logo-primary" />
        <div className="min-w-0 flex-1">
          <p className={sectionTitle}>
            {t(`settings.meeting.assets.${kind}.title`)}
          </p>
          <p className={sectionHint}>
            {t(`settings.meeting.assets.${kind}.description`)}
          </p>
        </div>
      </div>

      {asset ? (
        <div className="ml-8 space-y-2">
          <div className="rounded-md border border-mid-gray/20 bg-mid-gray/5 px-3 py-2">
            <p className="truncate text-sm font-medium">
              {asset.title ??
                asset.fileName ??
                t("settings.meeting.assets.pastedText")}
            </p>
            {asset.description && (
              <p className="line-clamp-2 text-xs text-text/60">
                {asset.description}
              </p>
            )}
            <p className="text-xs text-text/50">
              {t("settings.meeting.assets.summary", {
                count: asset.words,
                date: formatDate(asset.updatedAt, i18n.language),
              })}
            </p>
          </div>
          {tooLong && (
            <p className="text-xs text-yellow-600">
              {t("settings.meeting.assets.tooLong", {
                limit: wordLimit,
              })}
            </p>
          )}
          <div className="flex flex-wrap gap-2">
            <Button
              onClick={() => onEdit(kind)}
              variant="secondary"
              size="sm"
              className="flex items-center gap-2"
            >
              <Pencil className="h-3.5 w-3.5" />
              <span>{t("settings.meeting.assets.viewEdit")}</span>
            </Button>
            <Button
              onClick={() => void handleImport()}
              variant="secondary"
              size="sm"
              disabled={importing}
              className="flex items-center gap-2"
            >
              <FileUp className="h-3.5 w-3.5" />
              <span>{t("settings.meeting.assets.replace")}</span>
            </Button>
            <Button
              onClick={() => void handleClear()}
              variant="danger-ghost"
              size="sm"
              className="flex items-center gap-2"
            >
              <Trash2 className="h-3.5 w-3.5" />
              <span>{t("settings.meeting.assets.remove")}</span>
            </Button>
          </div>
        </div>
      ) : (
        <div className="ml-8 flex flex-wrap gap-2">
          <Button
            onClick={() => void handleImport()}
            variant="primary-soft"
            size="sm"
            disabled={importing}
            className="flex items-center gap-2"
          >
            <FileUp className="h-3.5 w-3.5" />
            <span>{t("settings.meeting.assets.import")}</span>
          </Button>
          <Button
            onClick={() => onEdit(kind)}
            variant="secondary"
            size="sm"
            className="flex items-center gap-2"
          >
            <ClipboardPaste className="h-3.5 w-3.5" />
            <span>{t("settings.meeting.assets.paste")}</span>
          </Button>
          <p className="w-full text-xs text-text/40">
            {t(`settings.meeting.assets.${kind}.formats`)}
          </p>
        </div>
      )}
    </div>
  );
};

interface TextAssetDialogProps {
  kind: TextAssetKind | null;
  onClose: () => void;
}

const TextAssetDialog: React.FC<TextAssetDialogProps> = ({ kind, onClose }) => {
  const { t } = useTranslation();
  // Sélecteurs : le chronomètre de l'enregistrement met le store à jour
  // plusieurs fois par seconde, sans rapport avec ce bloc.
  const assets = useMeetingStore((s) => s.assets);
  const applyAssets = useMeetingStore((s) => s.applyAssets);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (kind) setText(assets?.[kind]?.content ?? "");
    // Le texte n'est repris qu'à l'ouverture : une mise à jour de la liste
    // pendant la saisie n'écrase pas ce qui est tapé.
  }, [kind]);

  const handleSave = async () => {
    if (!kind) return;
    // Texte inchangé : rien à enregistrer, et le nom du fichier importé reste
    // affiché (un enregistrement le remplacerait par « Texte saisi »).
    if (text.trim() === (assets?.[kind]?.content ?? "").trim()) {
      onClose();
      return;
    }
    setSaving(true);
    try {
      if (applyAssets(await commands.meetingSetTextAsset(kind, text))) {
        onClose();
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={kind !== null}
      onOpenChange={(isOpen) => !isOpen && onClose()}
      title={kind ? t(`settings.meeting.assets.${kind}.title`) : ""}
      description={kind ? t(`settings.meeting.assets.${kind}.editHint`) : ""}
      closeLabel={t("common.close")}
      closeOnBackdrop={false}
      className="max-w-2xl"
      footer={
        <>
          <Button onClick={onClose} variant="secondary" size="md">
            {t("settings.meeting.assets.cancel")}
          </Button>
          <Button
            onClick={() => void handleSave()}
            variant="primary"
            size="md"
            disabled={saving || text.trim() === ""}
          >
            {t("settings.meeting.assets.save")}
          </Button>
        </>
      }
    >
      <Textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        className="h-[50vh] w-full select-text font-mono text-xs font-normal"
        spellCheck={false}
      />
    </Dialog>
  );
};

const ColorField: React.FC<{
  label: string;
  value: string;
  onChange: (value: string) => void;
}> = ({ label, value, onChange }) => (
  <label className="flex items-center gap-2 text-xs text-text/70">
    <input
      type="color"
      value={/^#[0-9a-f]{6}$/i.test(value) ? value : "#000000"}
      onChange={(e) => onChange(e.target.value.toUpperCase())}
      className="h-7 w-9 cursor-pointer rounded border border-mid-gray/40 bg-transparent p-0.5"
    />
    <span className="w-28">{label}</span>
    <input
      type="text"
      value={value}
      maxLength={7}
      onChange={(e) => onChange(e.target.value.toUpperCase())}
      aria-label={label}
      className="w-20 rounded-md border border-mid-gray/40 bg-mid-gray/10 px-2 py-1 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-logo-primary select-text"
    />
  </label>
);

// Charte graphique : appliquée par l'application au document du compte
// rendu (le modèle de langue n'en voit rien), d'où un aperçu immédiat.
const GraphicCharterBlock: React.FC = () => {
  const { t } = useTranslation();
  const assets = useMeetingStore((s) => s.assets);
  const applyAssets = useMeetingStore((s) => s.applyAssets);
  const saved = assets?.graphic ?? null;
  const [form, setForm] = useState<GraphicCharter>(saved ?? DEFAULT_CHARTER);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setForm(saved ?? DEFAULT_CHARTER);
  }, [saved]);

  const dirty = useMemo(
    () => JSON.stringify(form) !== JSON.stringify(saved ?? DEFAULT_CHARTER),
    [form, saved],
  );
  const validColors = [form.primaryColor, form.secondaryColor].every((c) =>
    /^#[0-9A-F]{6}$/i.test(c),
  );
  const logoSrc = assets?.logoPath ? convertFileSrc(assets.logoPath) : null;
  const configured = saved !== null || logoSrc !== null;

  const run = async (call: () => Promise<void>) => {
    setBusy(true);
    try {
      await call();
    } finally {
      setBusy(false);
    }
  };

  const handleLogo = () =>
    run(async () => {
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: t("settings.meeting.assets.graphic.logoType"),
            extensions: LOGO_EXTENSIONS,
          },
        ],
      });
      if (typeof selected === "string") {
        applyAssets(await commands.meetingImportLogo(selected));
      }
    });

  const handleSave = () =>
    run(async () => {
      applyAssets(await commands.meetingSaveGraphicCharter(form));
    });

  const handleClear = async () => {
    const confirmed = await ask(
      t("settings.meeting.assets.graphic.clearConfirm"),
      { title: t("settings.meeting.assets.graphic.title"), kind: "warning" },
    );
    if (confirmed) {
      await run(async () => {
        applyAssets(await commands.meetingClearGraphicCharter());
      });
    }
  };

  const update = (patch: Partial<GraphicCharter>) =>
    setForm((current) => ({ ...current, ...patch }));

  return (
    <div className="space-y-3 p-4">
      <div className="flex items-start gap-3">
        <Palette className="mt-0.5 h-5 w-5 shrink-0 text-logo-primary" />
        <div className="min-w-0 flex-1">
          <p className={sectionTitle}>
            {t("settings.meeting.assets.graphic.title")}
          </p>
          <p className={sectionHint}>
            {t("settings.meeting.assets.graphic.description")}
          </p>
        </div>
      </div>

      <div className="ml-8 grid gap-4 md:grid-cols-[1fr_220px]">
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <Button
              onClick={() => void handleLogo()}
              variant="secondary"
              size="sm"
              disabled={busy}
              className="flex items-center gap-2"
            >
              <ImagePlus className="h-3.5 w-3.5" />
              <span>
                {logoSrc
                  ? t("settings.meeting.assets.graphic.replaceLogo")
                  : t("settings.meeting.assets.graphic.addLogo")}
              </span>
            </Button>
            {logoSrc && (
              <Button
                onClick={() =>
                  void run(async () => {
                    applyAssets(await commands.meetingClearLogo());
                  })
                }
                variant="danger-ghost"
                size="sm"
                disabled={busy}
              >
                {t("settings.meeting.assets.graphic.removeLogo")}
              </Button>
            )}
          </div>
          <ColorField
            label={t("settings.meeting.assets.graphic.primary")}
            value={form.primaryColor}
            onChange={(primaryColor) => update({ primaryColor })}
          />
          <ColorField
            label={t("settings.meeting.assets.graphic.secondary")}
            value={form.secondaryColor}
            onChange={(secondaryColor) => update({ secondaryColor })}
          />
          <label className="flex items-center gap-2 text-xs text-text/70">
            <span className="w-[9.75rem] shrink-0">
              {t("settings.meeting.assets.graphic.font")}
            </span>
            <input
              list="meeting-fonts"
              value={form.font}
              maxLength={80}
              onChange={(e) => update({ font: e.target.value })}
              className="w-44 rounded-md border border-mid-gray/40 bg-mid-gray/10 px-2 py-1 text-xs focus:outline-none focus:ring-1 focus:ring-logo-primary select-text"
            />
            <datalist id="meeting-fonts">
              {FONTS.map((font) => (
                <option key={font} value={font} />
              ))}
            </datalist>
          </label>
          <label className="flex items-center gap-2 text-xs text-text/70">
            <span className="w-[9.75rem] shrink-0">
              {t("settings.meeting.assets.graphic.footer")}
            </span>
            <input
              value={form.footer}
              maxLength={300}
              placeholder={t(
                "settings.meeting.assets.graphic.footerPlaceholder",
              )}
              onChange={(e) => update({ footer: e.target.value })}
              className="min-w-0 flex-1 rounded-md border border-mid-gray/40 bg-mid-gray/10 px-2 py-1 text-xs placeholder:text-text/40 focus:outline-none focus:ring-1 focus:ring-logo-primary select-text"
            />
          </label>
          <div className="flex flex-wrap gap-2 pt-1">
            <Button
              onClick={() => void handleSave()}
              variant="primary-soft"
              size="sm"
              disabled={
                busy ||
                !validColors ||
                form.font.trim() === "" ||
                (!dirty && saved !== null)
              }
            >
              {t("settings.meeting.assets.graphic.save")}
            </Button>
            {configured && (
              <Button
                onClick={() => void handleClear()}
                variant="danger-ghost"
                size="sm"
                disabled={busy}
                className="flex items-center gap-2"
              >
                <Trash2 className="h-3.5 w-3.5" />
                <span>{t("settings.meeting.assets.graphic.clear")}</span>
              </Button>
            )}
          </div>
          {saved === null && (
            <p className="text-xs text-text/40">
              {t("settings.meeting.assets.graphic.notSaved")}
            </p>
          )}
        </div>

        {/* Aperçu : une page blanche quel que soit le thème de l'app, comme
            le document final. */}
        <div
          className="rounded-md border border-mid-gray/30 bg-white p-3 text-[#1F2937] shadow-sm"
          style={{ fontFamily: `"${form.font}", sans-serif` }}
          aria-label={t("settings.meeting.assets.graphic.preview")}
        >
          <div
            className="flex h-9 items-center justify-between border-b-2 pb-1"
            style={{ borderColor: validColors ? form.primaryColor : undefined }}
          >
            {logoSrc ? (
              <img
                src={logoSrc}
                alt=""
                className="h-7 max-w-[110px] object-contain"
              />
            ) : (
              <span className="text-[10px] text-gray-400">
                {t("settings.meeting.assets.graphic.previewLogo")}
              </span>
            )}
            <span
              className="text-[10px]"
              style={{ color: form.secondaryColor }}
            >
              {t("settings.meeting.assets.graphic.previewDate")}
            </span>
          </div>
          <p
            className="mt-2 text-sm font-bold leading-tight"
            style={{ color: form.primaryColor }}
          >
            {t("settings.meeting.assets.graphic.previewTitle")}
          </p>
          <p
            className="mt-2 text-[11px] font-semibold"
            style={{ color: form.secondaryColor }}
          >
            {t("settings.meeting.assets.graphic.previewSection")}
          </p>
          <p className="text-[11px] leading-snug text-gray-600">
            {t("settings.meeting.assets.graphic.previewBody")}
          </p>
          {form.footer.trim() && (
            <p className="mt-2 truncate border-t border-gray-200 pt-1 text-[9px] text-gray-400">
              {form.footer}
            </p>
          )}
        </div>
      </div>
    </div>
  );
};

export const ReportAssets: React.FC = () => {
  const { t } = useTranslation();
  const assets = useMeetingStore((s) => s.assets);
  const [editing, setEditing] = useState<TextAssetKind | null>(null);

  return (
    <div className="space-y-2">
      <div className="px-4">
        <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("settings.meeting.assets.title")}
        </h2>
        <p className="mt-1 text-sm text-text/60">
          {t("settings.meeting.assets.intro")}
        </p>
      </div>
      <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/10">
        <TextAssetBlock
          kind="editorial"
          asset={assets?.editorial ?? null}
          icon={BookOpenText}
          onEdit={setEditing}
        />
        <GraphicCharterBlock />
        <TextAssetBlock
          kind="skill"
          asset={assets?.skill ?? null}
          icon={Sparkles}
          onEdit={setEditing}
        />
      </div>
      <TextAssetDialog kind={editing} onClose={() => setEditing(null)} />
    </div>
  );
};
