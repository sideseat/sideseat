/**
 * File analysis sample demonstrating multimodal capabilities.
 *
 * Demonstrates:
 * - Image analysis (jpg, png, etc.) via multimodal content
 * - Document analysis (pdf) via DocumentBlock
 */

import { fileURLToPath } from 'url';
import * as path from 'path';
import * as fs from 'fs';
import { Agent, ImageBlock, TextBlock, DocumentBlock } from '@strands-agents/sdk';
import { resolveModel } from '../../shared/config.js';

// Content path: misc/samples/js/src/strands/samples -> misc/content
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const CONTENT_DIR = path.resolve(__dirname, '../../../../../content');

export async function run(modelId: string) {
  const imgPath = path.join(CONTENT_DIR, 'img.jpg');
  const pdfPath = path.join(CONTENT_DIR, 'task.pdf');

  if (!fs.existsSync(imgPath)) {
    throw new Error(`Image not found: ${imgPath}. Run from misc/samples/js directory.`);
  }
  if (!fs.existsSync(pdfPath)) {
    throw new Error(`PDF not found: ${pdfPath}. Run from misc/samples/js directory.`);
  }

  const imgBytes = fs.readFileSync(imgPath);
  const pdfBytes = fs.readFileSync(pdfPath);

  const agent = new Agent({
    model: resolveModel(modelId),
    printer: false,
    systemPrompt: 'You are a file analysis AI that can read images and documents.',
  });

  const textBlock = new TextBlock(
    `Analyze the attached image and PDF document. Describe the image contents, then summarize the tasks or instructions from the PDF.`
  );
  const imageBlock = new ImageBlock({
    format: 'jpeg',
    source: {
      bytes: new Uint8Array(imgBytes),
    },
  });
  const docBlock = new DocumentBlock({
    name: 'task',
    format: 'pdf',
    source: {
      bytes: new Uint8Array(pdfBytes),
    },
  });

  console.log('Analyzing image and PDF document...');
  console.log(`  Image: ${imgPath}`);
  console.log(`  PDF: ${pdfPath}`);
  console.log();

  const result = await agent.invoke([textBlock, imageBlock, docBlock]);
  console.log('Analysis Result:', result.toString());
}
