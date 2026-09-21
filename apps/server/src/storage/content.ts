import { createHash } from "node:crypto";
import { storage } from "./local.js";
import { AppError } from "../domain/store.js";
export async function revisionContent(data: any) {
  if (!data.storageKey) return { ...data, contentAvailability: "text" };
  const bytes = await storage.get(data.storageKey);
  if (
    bytes.length !== data.size ||
    createHash("sha256").update(bytes).digest("hex") !== data.checksum
  )
    throw new AppError(
      409,
      "Stored file does not match its immutable revision.",
    );
  if (bytes.length > 100000)
    return {
      ...data,
      contentAvailability: "download-only",
      contentReason:
        "File exceeds the 100 KB agent text retrieval limit. Attach a smaller text Resource.",
    };
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (text.includes("\0")) throw new Error("binary");
    return { ...data, text, contentAvailability: "text" };
  } catch {
    return {
      ...data,
      contentAvailability: "download-only",
      contentReason:
        "This binary file is stored safely but text extraction is not available. Attach a text or Markdown version for agents.",
    };
  }
}
