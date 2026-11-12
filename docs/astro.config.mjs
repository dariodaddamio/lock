import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'https://dariodaddamio.github.io',
  base: '/lock',
  integrations: [
    starlight({
      title: 'lock',
      description:
        "Freeze regions of a repository so coding agents cannot change them without a person's approval.",
      social: [
        {
          icon: 'github',
          label: 'GitHub',
          href: 'https://github.com/dariodaddamio/lock',
        },
      ],
      editLink: {
        baseUrl: 'https://github.com/dariodaddamio/lock/edit/main/docs/',
      },
      sidebar: [
        { label: 'Home', link: '/' },
        { label: 'Using lock', link: '/using/' },
        { label: 'How it works', link: '/how-it-works/' },
        { label: 'Limits', link: '/limits/' },
        { label: 'Commands', link: '/commands/' },
        { label: 'Lock markers', link: '/markers/' },
        { label: 'Team setup', link: '/team/' },
        { label: 'Questions', link: '/faq/' },
        {
          label: 'Project',
          items: [
            { label: 'Releases', link: 'https://github.com/dariodaddamio/lock/releases' },
            {
              label: 'Changelog',
              link: 'https://github.com/dariodaddamio/lock/blob/main/CHANGELOG.md',
            },
            {
              label: 'Contributing',
              link: 'https://github.com/dariodaddamio/lock/blob/main/CONTRIBUTING.md',
            },
            {
              label: 'Security',
              link: 'https://github.com/dariodaddamio/lock/blob/main/SECURITY.md',
            },
          ],
        },
      ],
    }),
  ],
});
