export function syncConflictTarget(path: string): { id: string } | { title: string } | null {
  const parts = path.split("/");
  if (parts.some(part => !part || part === "." || part === ".." || part.includes("\\"))) return null;
  const book = /^books\/([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})\/(?:original\.(?:epub|fb2|mobi|azw3|pdf)|book\.json|position\.json)$/i.exec(path);
  if (book) return { id: book[1] };
  if (parts.includes("assets") || !path.endsWith(".md")) return null;
  if (parts[0] === "pages" || parts[0] === "journals") return { title: parts.slice(1).join("/").slice(0, -3) };
  if (parts[0] === "knowledge") return { title: `Knowledge/${parts.slice(1).join("/").slice(0, -3)}` };
  return null;
}
