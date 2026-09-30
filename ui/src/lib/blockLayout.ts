export function getHeadingLevel(content: string): number {
  const match = content.trimStart().match(/^(#{1,6})\s+/);
  return match ? match[1].length : 0;
}

export function getBulletMinHeight(content: string): string {
  switch (getHeadingLevel(content)) {
    case 1:
      return "32px";
    case 2:
      return "28px";
    case 3:
      return "25px";
    default:
      return "24px";
  }
}
