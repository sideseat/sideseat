/**
 * Image generation and critic evaluation sample.
 *
 * Artist agent generates images via tool calls, then critic agent
 * evaluates them using an image_reader tool with toModelOutput
 * to return images as native content parts (not base64-as-text).
 */

import { generateText, tool, stepCountIs } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';
import { generateImage } from '../../shared/tools/generate-image.js';
import { readImage } from '../../shared/tools/read-image.js';

const ARTIST_SYSTEM_PROMPT =
  'You will be instructed to generate a number of images of a given subject. ' +
  'Vary the prompt for each generated image to create a variety of options. ' +
  'Your final output must contain ONLY a comma-separated list of the filesystem paths of generated images.';

const CRITIC_SYSTEM_PROMPT =
  'You will be provided with a list of filesystem paths, each containing an image. ' +
  'Describe each image, and then choose which one is best. ' +
  'Your final line of output must be as follows: ' +
  'FINAL DECISION: <path to final decision image>';

export async function run(modelId: string) {
  const model = bedrock(resolveModel(modelId));

  const imageGenTool = tool({
    description: 'Generate an image from a text prompt. Returns the filesystem path.',
    inputSchema: z.object({
      prompt: z.string().describe('Detailed image description'),
    }),
    execute: async ({ prompt }) => {
      const result = await generateImage({ prompt });
      return result.path;
    },
  });

  const imageReaderTool = tool({
    description: 'Read and analyze an image from a filesystem path.',
    inputSchema: z.object({
      path: z.string().describe('Filesystem path to the image'),
    }),
    execute: async ({ path }) => {
      return await readImage({ path });
    },
    toModelOutput({ output }) {
      return {
        type: 'content' as const,
        value: [
          { type: 'text' as const, text: `Image: ${output.path}` },
          {
            type: 'media' as const,
            data: output.base64,
            mediaType: output.mimeType,
          },
        ],
      };
    },
  });

  // Artist agent that generates images based on prompts
  console.log('Artist generating images...');
  const artistResult = await generateText({
    model,
    tools: { generate_image: imageGenTool },
    stopWhen: stepCountIs(5),
    system: ARTIST_SYSTEM_PROMPT,
    prompt: 'Generate 3 images of a dog',
    experimental_telemetry: { isEnabled: true },
  });

  console.log('Artist result:', artistResult.text);

  // Critic agent that evaluates and selects the best image
  console.log('\nCritic evaluating images...');
  const criticResult = await generateText({
    model,
    tools: { image_reader: imageReaderTool },
    stopWhen: stepCountIs(5),
    system: CRITIC_SYSTEM_PROMPT,
    prompt: artistResult.text,
    experimental_telemetry: { isEnabled: true },
  });

  console.log('Critic result:', criticResult.text);
}
