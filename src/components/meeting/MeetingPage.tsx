import React, { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useMeetingStore } from "@/stores/meetingStore";
import { RecorderPanel } from "./RecorderPanel";
import { RecordingList } from "./RecordingList";
import { ReportAssets } from "./ReportAssets";

// Onglet « Réunion » : enregistrer le micro sans limite de durée, retrouver
// ses enregistrements, et préparer ce qu'appliquera le compte rendu.
export const MeetingPage: React.FC = () => {
  const { t } = useTranslation();
  const initialize = useMeetingStore((s) => s.initialize);

  useEffect(() => {
    void initialize();
  }, [initialize]);

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.meeting.title")}
          </h2>
          <p className="text-sm text-text/60 mt-1">
            {t("settings.meeting.description")}
          </p>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg p-4">
          <RecorderPanel />
        </div>
      </div>
      <RecordingList />
      <ReportAssets />
    </div>
  );
};
