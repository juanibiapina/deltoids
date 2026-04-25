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
		}),
	],
});
