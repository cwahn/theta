import fs from "fs";
import path from "path";

export interface ActorInfo {
  /** Actor name, e.g. "ChatRoom" */
  name: string;
  /** Ref class name, e.g. "ChatRoomRef" */
  refName: string;
  /** View type name, e.g. "ChatRoomView" */
  viewName: string;
}

/**
 * Parse actor information from the wasm-pack generated `.d.ts` file.
 * Detects actors by matching `export interface XxxRef { ... prep(): Promise<XxxView> ... }`.
 */
export function parseActors(pkgDir: string, moduleName: string): ActorInfo[] {
  const dtsPath = path.join(pkgDir, `${moduleName}.d.ts`);
  if (!fs.existsSync(dtsPath)) return [];

  const content = fs.readFileSync(dtsPath, "utf-8");
  const actors: ActorInfo[] = [];
  const pattern = /export interface (\w+)Ref \{[\s\S]*?prep\(\): Promise<(\w+)>/g;

  let match;
  while ((match = pattern.exec(content)) !== null) {
    actors.push({
      name: match[1],
      refName: match[1] + "Ref",
      viewName: match[2],
    });
  }

  return actors;
}
