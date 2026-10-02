import { invoke } from "@tauri-apps/api/core";

export type StudyKind = "page" | "book" | "flashcards" | "audio" | "video" | "youtube" | "website" | "library";
export interface StudyProgress {
  position: number;
  total: number;
  anchor: string;
  label: string;
}
export interface StudyItem {
  id: string;
  title: string;
  topic: string;
  kind: StudyKind;
  source: string;
  progress: StudyProgress;
  createdAt: string;
  updatedAt: string;
}
export interface StudyDay {
  itemId: string;
  topic: string;
  day: string;
  seconds: number;
}
export interface StudySnapshot {
  items: StudyItem[];
  days: StudyDay[];
  topics?: string[];
}

export const listStudies = (graphPath: string) =>
  invoke<StudySnapshot>("list_studies", { graphPath });
export const saveStudy = (graphPath: string, item: StudyItem) =>
  invoke<StudyItem>("save_study", { graphPath, item });
export const removeStudy = (graphPath: string, id: string) =>
  invoke<void>("remove_study", { graphPath, id });

export const fetchStudyLinkTitle = (url: string): Promise<string> =>
  invoke<string>("study_link_title", { url });

export async function recordStudyActivity(
  graphPath: string, id: string, seconds: number, day: string, progress: StudyProgress | null,
  requestId: string = crypto.randomUUID(),
): Promise<void> {
  const args = { graphPath, id, seconds, day, progress: progress ? { ...progress } : null,
    requestId };
  try {
    await invoke<void>("record_study_activity", args);
  } catch {
    // A lost IPC response may follow a successful commit. Reuse the receipt ID
    // so retrying cannot count time twice or overwrite newer progress.
    await invoke<void>("record_study_activity", args);
  }
}
