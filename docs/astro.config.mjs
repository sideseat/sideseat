// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightClientMermaid from '@pasqal-io/starlight-client-mermaid';

// https://astro.build/config
export default defineConfig({
  site: 'https://sideseat.ai',
  integrations: [
    starlight({
      title: 'SideSeat',
      logo: {
        src: './src/assets/favicon.png',
      },
      favicon: '/favicon.ico',
      plugins: [starlightClientMermaid()],
      customCss: ['./src/styles/custom.css'],
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/sideseat/sideseat' }],
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { label: 'Overview', link: '/docs/' },
            { label: 'First Run', slug: 'docs/quickstart' },
            { label: 'MCP Server', slug: 'docs/mcp' },
            { label: 'Troubleshooting', slug: 'docs/troubleshooting' },
          ],
        },
        {
          label: 'Integrations',
          items: [
            { label: 'Overview', link: '/docs/integrations/' },
            {
              label: 'Frameworks',
              items: [
                { label: 'Strands Agents', slug: 'docs/integrations/frameworks/strands' },
                { label: 'Vercel AI SDK', slug: 'docs/integrations/frameworks/vercel-ai' },
                { label: 'LangGraph', slug: 'docs/integrations/frameworks/langgraph' },
                { label: 'CrewAI', slug: 'docs/integrations/frameworks/crewai' },
                { label: 'AutoGen', slug: 'docs/integrations/frameworks/autogen' },
                { label: 'Google ADK', slug: 'docs/integrations/frameworks/google-adk' },
                { label: 'OpenAI Agents SDK', slug: 'docs/integrations/frameworks/openai-agents' },
                { label: 'Other Frameworks', slug: 'docs/integrations/frameworks/other' },
              ],
            },
            {
              label: 'Providers',
              items: [
                { label: 'OpenAI', slug: 'docs/integrations/providers/openai' },
                { label: 'Anthropic', slug: 'docs/integrations/providers/anthropic' },
                { label: 'Amazon Bedrock', slug: 'docs/integrations/providers/bedrock' },
                { label: 'Azure OpenAI', slug: 'docs/integrations/providers/azure' },
                { label: 'Google Vertex AI', slug: 'docs/integrations/providers/vertex' },
              ],
            },
          ],
        },
        {
          label: 'SDKs',
          items: [
            {
              label: 'Python',
              items: [
                { label: 'Overview', link: '/docs/sdks/python/' },
                { label: 'Configuration', slug: 'docs/sdks/python/configuration' },
                { label: 'SideSeat Class', slug: 'docs/sdks/python/telemetry' },
                { label: 'Exporters', slug: 'docs/sdks/python/exporters' },
              ],
            },
            {
              label: 'JavaScript',
              items: [
                { label: 'Overview', link: '/docs/sdks/javascript/' },
                { label: 'Configuration', slug: 'docs/sdks/javascript/configuration' },
                { label: 'init() / createClient()', slug: 'docs/sdks/javascript/init' },
              ],
            },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Core Concepts', slug: 'docs/concepts' },
            {
              label: 'API',
              items: [
                { label: 'Overview', link: '/docs/reference/api/' },
                { label: 'Traces', slug: 'docs/reference/api/traces' },
                { label: 'Spans', slug: 'docs/reference/api/spans' },
                { label: 'Sessions', slug: 'docs/reference/api/sessions' },
                { label: 'SSE Streaming', slug: 'docs/reference/api/sse' },
              ],
            },
            { label: 'CLI Reference', slug: 'docs/reference/cli' },
            { label: 'Configuration Schema', slug: 'docs/reference/config' },
            { label: 'OpenTelemetry', slug: 'docs/reference/otel' },
            { label: 'Authentication', slug: 'docs/reference/auth' },
            { label: 'Storage Manager', slug: 'docs/reference/storage' },
            { label: 'Secret Manager', slug: 'docs/reference/secrets' },
          ],
        },
        {
          label: 'Advanced',
          collapsed: true,
          items: [
            { label: 'Architecture', slug: 'docs/architecture' },
          ],
        },
      ],
    }),
  ],
});
