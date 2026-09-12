import * as fs from 'fs';
import * as path from 'path';

export interface ReadImageOptions {
  path: string;
}

export interface ReadImageResult {
  path: string;
  base64: string;
  mimeType: string;
}

export async function readImage(options: ReadImageOptions): Promise<ReadImageResult> {
  const imagePath = options.path;

  if (!fs.existsSync(imagePath)) {
    throw new Error(`Image not found: ${imagePath}`);
  }

  const ext = path.extname(imagePath).toLowerCase();
  const mimeTypes: Record<string, string> = {
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.png': 'image/png',
    '.gif': 'image/gif',
    '.webp': 'image/webp',
  };

  const mimeType = mimeTypes[ext] ?? 'image/png';
  const base64 = fs.readFileSync(imagePath).toString('base64');

  return { path: imagePath, base64, mimeType };
}
