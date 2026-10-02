export interface CFIPart {
  index: number;
  id?: string;
  offset?: number;
  temporal?: number;
  spatial?: number[];
  text?: string[];
  side?: string;
}

export type CFIPath = CFIPart[][] & { parent?: undefined };
export interface CFIRange {
  parent: CFIPath;
  start: CFIPath;
  end: CFIPath;
}
export type ParsedCFI = CFIPath | CFIRange;
export type CFIFilter = (node: Node) => number;

export function parse(cfi: string): ParsedCFI;
export function joinIndir(...cfis: string[]): string;
export function fromRange(range: Range, filter?: CFIFilter): string;
export function toRange(doc: Document, parts: ParsedCFI, filter?: CFIFilter): Range;
export const fake: {
  fromIndex(index: number): string;
  toIndex(parts: CFIPart[] | undefined): number;
};
