/**
 * File analysis sample demonstrating multimodal capabilities.
 *
 * Demonstrates:
 * - Image analysis (jpg, png, etc.) via image_reader tool
 * - Document analysis (pdf) via document content
 */

import { generateText, tool, stepCountIs } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { resolveModel } from '../../shared/config.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_DIR = path.resolve(__dirname, '../../../../../content');

// Image reader tool that reads an image from disk and returns base64 data
const imageReader = tool({
  description: 'Read and analyze an image from a file path.',
  inputSchema: z.object({
    path: z.string().describe('The file path to the image'),
  }),
  execute: async ({ path: imgPath }) => {
    if (!fs.existsSync(imgPath)) {
      return { error: `Image not found: ${imgPath}` };
    }

    const imgBase64 = fs.readFileSync(imgPath).toString('base64');
    const ext = path.extname(imgPath).toLowerCase().slice(1);
    const mediaType = ext === 'jpg' ? 'image/jpeg' : `image/${ext}`;

    return {
      type: 'image',
      data: imgBase64,
      mediaType,
    };
  },
});

export async function run(modelId: string) {
  const imgPath = path.join(CONTENT_DIR, 'img.jpg');
  const pdfPath = path.join(CONTENT_DIR, 'task.pdf');

  if (!fs.existsSync(imgPath)) {
    throw new Error(`Image not found: ${imgPath}. Run from misc/samples/js directory.`);
  }
  if (!fs.existsSync(pdfPath)) {
    throw new Error(`PDF not found: ${pdfPath}. Run from misc/samples/js directory.`);
  }

  const pdfBase64 = fs.readFileSync(pdfPath).toString('base64');

  console.log('Analyzing image with PDF instructions...');
  console.log(`  Image: ${imgPath}`);
  console.log(`  PDF: ${pdfPath}`);
  console.log();

  const { text } = await generateText({
    model: bedrock(resolveModel(modelId)),
    tools: { image_reader: imageReader },
    system: 'You are a file analysis AI that can read images and documents.',
    stopWhen: stepCountIs(5),
    messages: [
      {
        role: 'user',
        content: [
          {
            type: 'text',
            text: `Read the image '${imgPath}'. Describe its contents in detail using instructions from PDF.`,
          },
          {
            type: 'file',
            data: pdfBase64,
            mediaType: 'application/pdf',
          },
        ],
      },
    ],
    experimental_telemetry: { isEnabled: true },
  });

  console.log('Analysis Result:', text);
}
