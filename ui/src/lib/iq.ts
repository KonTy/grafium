import { invoke } from "@tauri-apps/api/core";
import type { AnkiImportSummary } from "./api";

export interface IqNorms {
  citation: string;
  population: string;
  sampleSize: number;
  ageMin: number;
  ageMax: number;
  language: string;
  testVersion: string;
  itemSetHash: string;
  timeLimitSeconds: number | null;
  instructions: string;
  zScores: number[];
}
export interface IqPackSummary {
  id: string;
  title: string;
  itemCount: number;
  norms: IqNorms | null;
}
export interface IqParticipant { age: number; language: string; unseen: boolean }
export interface IqSession {
  id: string;
  graphPath: string;
  title: string;
  questions: string[];
  expiresAt: number | null;
}
export interface IqResult {
  sessionId: string;
  title: string;
  correct: number;
  total: number;
  unanswered: number;
  percentage: number;
  elapsedSeconds: number;
  estimatedIq: number | null;
  iqUnavailableReason: string | null;
  normCitation: string | null;
  items: { question: string; answer: string; acceptedAnswers: string[]; explanation: string; correct: boolean }[];
}

export const listIqPacks = () => invoke<IqPackSummary[]>("list_iq_packs");
export const importFlashcardPack = (path: string) =>
  invoke<{ anki: AnkiImportSummary | null; iq: IqPackSummary | null }>("import_flashcard_pack", { path });
export const startIqSession = (packId: string, participant: IqParticipant) =>
  invoke<IqSession>("start_iq_session", { packId, participant });
export const answerIqItem = (session: IqSession, index: number, answer: string) =>
  invoke<void>("answer_iq_item", { graphPath: session.graphPath, sessionId: session.id, index, answer });
export const finishIqSession = (session: IqSession) =>
  invoke<IqResult>("finish_iq_session", { graphPath: session.graphPath, sessionId: session.id });
