export function artistPortraitInitials(name: string): string | null {
  const words = name.normalize("NFC").trim().split(/\s+/u).filter(word => /[\p{L}\p{N}]/u.test(word));
  if (!words.length) return null;
  return [words[0], ...(words.length > 1 ? [words[words.length - 1]] : [])]
    .map(word => Array.from(word).find(char => /[\p{L}\p{N}]/u.test(char))!).join("").toLocaleUpperCase();
}
