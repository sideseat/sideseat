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
      head: [
        {
          tag: 'script',
          content: `
            !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(".");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement("script")).type="text/javascript",p.async=!0,p.src=s.api_host.replace(".i.posthog.com","-assets.i.posthog.com")+"/static/array.js",(r=t.getElementsByTagName("script")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a="posthog",u.people=u.people||[],u.toString=function(t){var e="posthog";return"posthog"!==a&&(e+="."+a),t||(e+=" (stub)"),e},u.people.toString=function(){return u.toString(1)+".people (stub)"},o="init capture register register_once register_for_session unregister opt_out_capturing has_opted_out_capturing opt_in_capturing reset isFeatureEnabled getFeatureFlag getFeatureFlagPayload reloadFeatureFlags group identify setPersonProperties setPersonPropertiesForFlags resetPersonPropertiesForFlags setGroupPropertiesForFlags resetGroupPropertiesForFlags resetGroups onFeatureFlags addFeatureFlagsHandler onSessionId getSurveys getActiveMatchingSurveys renderSurvey canRenderSurvey getNextSurveyStep".split(" "),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);
            posthog.init('phc_qmVEhL6Wwr9JOUN1ZQHu7qJJ83OmAuQqDk9MDl2dfqJ', {
              api_host: 'https://us.i.posthog.com',
              defaults: '2026-01-30'
            })
          `,
        },
      ],
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
