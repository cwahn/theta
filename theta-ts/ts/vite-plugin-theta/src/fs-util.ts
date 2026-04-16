import fs from "fs";

/** Write `content` to `filePath` only if the file's current content differs. Preserves mtime on no-op. */
export function writeIfChanged(filePath: string, content: string): void {
  const existing = fs.existsSync(filePath) ? fs.readFileSync(filePath, "utf8") : null;
  if (existing !== content) fs.writeFileSync(filePath, content);
}
