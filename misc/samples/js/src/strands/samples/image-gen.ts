/**
 * Image generation and critic evaluation sample.
 *
 * Demonstrates a multi-agent workflow:
 * 1. Artist agent generates multiple images based on prompts
 * 2. Critic agent evaluates and selects the best image
 */

import { Agent, tool } from '@strands-agents/sdk';
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

  // Image reader tool (returns base64 for multimodal analysis)
  const imageReaderTool = tool({
    name: 'image_reader',
    description: 'Read an image file and return its contents for analysis.',
    inputSchema: z.object({
      path: z.string().describe('Filesystem path to the image file'),
    }),
    callback: ({ path: filePath }): Record<string, string> => {
      if (!fs.existsSync(filePath)) {
        return { error: `File not found: ${filePath}` };
      }
      const base64 = fs.readFileSync(filePath).toString('base64');
      return {
        type: 'image',
        data: base64,
        mediaType: 'image/png',
        path: filePath,
      };
    },
  });

  // Artist agent that generates images based on prompts
  const artist = new Agent({
    model,
    tools: [imageGenTool],
    systemPrompt: `You will be instructed to generate a number of images of a given subject.
Vary the prompt for each generated image to create a variety of options.
Your final output must contain ONLY a comma-separated list of the filesystem paths of generated images.`,
  });

  // Critic agent that evaluates and selects the best image
  const critic = new Agent({
    model,
    tools: [imageReaderTool],
    systemPrompt: `You will be provided with a list of filesystem paths, each containing an image.
Describe each image, and then choose which one is best.
Your final line of output must be as follows:
FINAL DECISION: <path to final decision image>`,
  });

  // Generate multiple images using the artist agent
  console.log('Artist generating images...');
  const artistResult = await artist.invoke('Generate 3 images of a dog');
  console.log(`Artist result: ${artistResult}`);

  // Pass the image paths to the critic agent for evaluation
  console.log('\nCritic evaluating images...');
  const criticResult = await critic.invoke(String(artistResult));
  console.log(`Critic result: ${criticResult}`);
}
