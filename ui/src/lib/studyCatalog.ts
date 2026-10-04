import type { FlashcardTopic, PageSummary } from "./api";
import type { StudyItem } from "./studies";
import type { ReaderBook } from "./privateReader";
import { fuzzyRank } from "./fuzzy";
import { studyKindLabels, studyMediaKind } from "./studySources";

export type StudySourceChoice = Pick<StudyItem, "kind" | "source" | "title">;
export interface StudyCandidate extends StudySourceChoice {
  key: string;
  detail: string;
}

export function isStudySourceAdded(items: StudyItem[], candidate: StudySourceChoice): boolean {
  return items.some(item => item.source === candidate.source && (item.kind === candidate.kind
    || (["page", "book"].includes(item.kind) && ["page", "book"].includes(candidate.kind))));
}

export function studyCatalog(pages: PageSummary[], topics: FlashcardTopic[], assets: string[]): StudyCandidate[] {
  return [
    ...pages.map(page => ({
      key: `page:${page.id}`, title: page.title, source: page.id,
      kind: page.is_book ? "book" as const : "page" as const,
      detail: page.is_journal ? "Journal page" : page.is_book ? "Original book" : "Graph page",
    })),
    ...topics.map(topic => ({
      key: `cards:${topic.topic}`, title: topic.topic ? `#${topic.topic}` : "Untagged cards",
      kind: "flashcards" as const, source: topic.topic,
      detail: `${topic.total} cards · ${topic.due} due`,
    })),
    ...assets.flatMap(path => {
      const kind = studyMediaKind(path);
      return kind ? [{
        key: `asset:${path}`, title: path.split("/").at(-1)!.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ") || path,
        kind, source: path, detail: path,
      }] : [];
    }),
  ];
}

export function libraryStudyCatalog(books: ReaderBook[]): StudyCandidate[] {
  return books.map(book => ({
    key: `library:${book.id}`, title: book.title, source: book.id, kind: "library",
    detail: book.available ? `Library · ${book.kind.toUpperCase()}` : book.disconnected ? "Library · Disconnected" : "Library · Source unavailable",
  }));
}

export function searchStudyCatalog(candidates: StudyCandidate[], query: string): StudyCandidate[] {
  const text = query.trim().replace(/^\[\[(.*)\]\]$/, "$1").replace(/^#/, "");
  return fuzzyRank(candidates, text, item => `${item.title} ${studyKindLabels[item.kind]} ${item.detail}`);
}

export function isStudyLinkInput(value: string): boolean {
  return /^(?:https?:|www\.|[a-z][a-z\d+.-]*:\/\/|(?:javascript|data|file):)/i.test(value.trim());
}
