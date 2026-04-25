// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://deltoids.dev',
	integrations: [
		starlight({
			title: 'deltoids',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/juanibiapina/deltoids' }],
			sidebar: [],
			pagefind: false,
			customCss: ['./src/styles/landing.css'],
			expressiveCode: {
				// `night-owl` (default dark) renders shell args in #3B61B0 which
				// fails WCAG AA on the light-mode codeblock background.
				// `github-light` is high-contrast and pairs visually with night-owl.
				themes: ['night-owl', 'github-light'],
			},
		}),
	],
});
