/**
 * Image generation and critic evaluation sample.
 *
 * Demonstrates a multi-agent workflow:
 * 1. Artist agent generates multiple images based on prompts
 * 2. Critic agent evaluates and selects the best image
 */

import { Agent, tool, ImageBlock, TextBlock } from '@strands-agents/sdk';
import { z } from 'zod';
import * as fs from 'fs';
import { resolveModel } from '../../shared/config.js';
import { generateImage } from '../../shared/tools/generate-image.js';

export async function run(modelId: string) {
  const model = resolveModel(modelId);

  // Image generation tool
  const imageGenTool = tool({
    name: 'generate_image',
    description: 'Generate an image from a text prompt. Returns the filesystem path.',
    inputSchema: z.object({
      prompt: z.string().describe('Detailed image description'),
    }),
    callback: async ({ prompt }) => {
      const result = await generateImage({ prompt });
      return result.path;
    },
  });

  // Artist agent that generates images based on prompts
  const artist = new Agent({
    model,
    tools: [imageGenTool],
    printer: false,
    systemPrompt: `You will be instructed to generate a number of images of a given subject.
Vary the prompt for each generated image to create a variety of options.
Your final output must contain ONLY a comma-separated list of the filesystem paths of generated images.`,
  });

  // Critic agent that evaluates and selects the best image
  const critic = new Agent({
    model,
    printer: false,
    systemPrompt: `You will be provided with a set of images.
Describe each image, and then choose which one is best.
Your final line of output must be as follows:
FINAL DECISION: <index of chosen image, 1-based>`,
  });

  // Generate multiple images using the artist agent
  console.log('Artist generating images...');
  const artistResult = await artist.invoke('Generate 3 images of a dog');
  console.log(`Artist result: ${artistResult.toString()}`);

  // Parse paths and load images as native ImageBlock objects
  const imagePaths = artistResult
    .toString()
    .split(',')
    .map((p) => p.trim())
    .filter(Boolean);

  const content = [
    new TextBlock(
      `You will be provided with ${imagePaths.length} images.\n` +
        `Describe each image, and then choose which one is best.\n` +
        `Your final line of output must be as follows:\nFINAL DECISION: <index of chosen image, 1-based>`
    ),
    ...imagePaths.map(
      (p) =>
        new ImageBlock({
          format: 'png',
          source: { bytes: new Uint8Array(fs.readFileSync(p)) },
        })
    ),
  ];

  // Pass images directly to the critic agent for evaluation
  console.log('\nCritic evaluating images...');
  const criticResult = await critic.invoke(content);
  console.log(`Critic result: ${criticResult.toString()}`);
}
