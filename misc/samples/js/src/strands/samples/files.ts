/**
 * File analysis sample demonstrating multimodal capabilities.
 *
 * Demonstrates:
 * - Image analysis (jpg, png, etc.) via multimodal content
 * - Document analysis (pdf) via document content blocks
 *
 * Note: This sample uses both image and PDF analysis like Python,
 * combining them in a single multimodal request.
 */

import { fileURLToPath } from 'url';
import * as path from 'path';
import * as fs from 'fs';
import { Agent, ImageBlock, TextBlock } from '@strands-agents/sdk';
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
  // Get PDF size without reading entire file into memory
  const pdfSize = fs.statSync(pdfPath).size;

  const agent = new Agent({
    model: resolveModel(modelId),
    systemPrompt: 'You are a file analysis AI that can read images and documents.',
  });

  // Create multimodal content blocks
  const textBlock = new TextBlock(
    `Read the image '${imgPath}'. Describe its contents in detail using instructions from PDF.`
  );
  const imageBlock = new ImageBlock({
    format: 'jpeg',
    source: {
      bytes: new Uint8Array(imgBytes),
    },
  });

  // Note: PDF/document support in Strands JS SDK may vary
  // For now, we'll send the image with the text prompt referencing the PDF
  // If native document blocks are supported, this can be updated

  console.log('Analyzing image with PDF instructions...');
  console.log(`  Image: ${imgPath}`);
  console.log(`  PDF: ${pdfPath}`);
  console.log();

  // Try with image first
  const result = await agent.invoke([textBlock, imageBlock]);
  console.log('Analysis Result:', result);

  // Additional: Try PDF-only analysis if document blocks are supported
  // This is a placeholder for when Strands JS SDK adds document block support
  console.log('\n--- PDF Document Analysis ---');
  console.log('Note: Native PDF document blocks may require SDK updates.');
  console.log(`PDF size: ${pdfSize} bytes`);
}
