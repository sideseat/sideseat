export const FILE_URI_PREFIX = "#!B64!#";

const FILE_HASH_PATTERN = /^[0-9a-f]{64}$/i;

export interface FileUri {
  hash: string;
  mediaType?: string;
}

export function parseFileUri(value: string): FileUri | null {
  if (!value.startsWith(FILE_URI_PREFIX)) {
    return null;
  }

  const payload = value.slice(FILE_URI_PREFIX.length);
  const separatorIndex = payload.indexOf("::");
  if (separatorIndex === -1) {
    return null;
  }

  const hash = payload.slice(separatorIndex + 2);
  if (!FILE_HASH_PATTERN.test(hash)) {
    return null;
  }

  const mediaType = payload.slice(0, separatorIndex);
  return mediaType ? { hash, mediaType } : { hash };
}
