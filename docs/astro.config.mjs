// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightClientMermaid from '@pasqal-io/starlight-client-mermaid';

// https://astro.build/config
export default defineConfig({
  integrations: [
    starlight({
      title: 'SideSeat Docs',
      plugins: [starlightClientMermaid()],
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/spugachev/sideseat' }],
      sidebar: [
        {
          label: 'Guides',
          items: [{ label: 'Getting Started', slug: 'guides/example' }],
        },
        {
          label: 'Reference',
          autogenerate: { directory: 'reference' },
        },
      ],
    }),
  ],
});
