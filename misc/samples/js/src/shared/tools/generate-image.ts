import { InvokeModelCommand } from '@aws-sdk/client-bedrock-runtime';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { getBedrockClient } from '../aws-client.js';
import { config } from '../config.js';

// Output directory: misc/samples/js/output/
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const OUTPUT_DIR = path.resolve(__dirname, '../../../output');

export interface GenerateImageOptions {
  prompt: string;
  negativePrompt?: string;
  width?: number;
  height?: number;
  seed?: number;
}

export interface GenerateImageResult {
  path: string;
  base64: string;
}

export async function generateImage(options: GenerateImageOptions): Promise<GenerateImageResult> {
  const client = getBedrockClient();

  // Ensure output directory exists
  if (!fs.existsSync(OUTPUT_DIR)) {
    fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  }

  // Bedrock image models take incompatible request bodies. Stability uses a flat
  // prompt/mode shape; Amazon's Titan and Nova Canvas use taskType/textToImageParams.
  // Both return the PNG in `images[0]`, so only the request differs.
  const modelId = config.models.imageGen;
  const body = modelId.startsWith('stability.')
    ? {
        prompt: options.prompt,
        mode: 'text-to-image',
        output_format: 'png',
        ...(options.negativePrompt && { negative_prompt: options.negativePrompt }),
        ...(options.seed !== undefined && { seed: options.seed }),
      }
    : {
        taskType: 'TEXT_IMAGE',
        textToImageParams: {
          text: options.prompt,
          ...(options.negativePrompt && { negativeText: options.negativePrompt }),
        },
        imageGenerationConfig: {
          numberOfImages: 1,
          width: options.width ?? 512,
          height: options.height ?? 512,
          cfgScale: 8.0,
          ...(options.seed !== undefined && { seed: options.seed }),
        },
      };

  const command = new InvokeModelCommand({
    modelId,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });

  const response = await client.send(command);
  const result = JSON.parse(new TextDecoder().decode(response.body));
  const base64 = result.images[0];

  // Save to file
  const filename = `image_${Date.now()}.png`;
  const filepath = path.join(OUTPUT_DIR, filename);
  fs.writeFileSync(filepath, Buffer.from(base64, 'base64'));

  return { path: filepath, base64 };
}
