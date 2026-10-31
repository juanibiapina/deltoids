# Changelog

All notable changes to the browser PR reviewer (review.deltoids.dev) are
documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

- Sign in with GitHub from the top bar. Reviewing still works without signing in.
- Add your open pull requests to the home page when signed in: the ones you created, are assigned to, are mentioned in, or are asked to review. Click one to review it. The "deltoids review" name in the top bar brings you back home.
- The file tree highlights the file you are currently reading and follows along as you scroll, so you always know where you are in the pull request. The highlighted row stays in view within the tree.
- Hunks are separated by a divider showing how many unchanged lines are hidden — above the first hunk, between hunks, and below the last. Click a divider to reveal that code in place; the hunks then merge into one continuous view with no repeated header.
- Grow a hunk's context to its enclosing function or class by clicking or tapping its header, or with the z and x keys, like the TUI.
- Try a phone layout under Layout in the display settings: a full-height file list you drag down to close, and a sticky file path that reopens it.
- Pick the syntax highlighting theme from a toolbar selector (grouped into dark and light themes, or "Auto" to follow the page theme). Your choice is remembered, and first-time visitors get Tokyo Night on dark or GitHub on light. Switching recolors the open diff instantly without re-fetching the pull request.

### Changed

- Signing in with GitHub raises GitHub's limit to 5000 requests per hour and lets you review private repositories you have access to.
- The scrollbars throughout the reviewer are now thin and theme-tinted instead of the chunky default ones.

### Fixed

- Fix the diff jumping while you read when files above finish loading, a hidden gap above expands, or the theme changes. This now also holds in Safari.

### Removed

- Remove the 🔑 button for pasting a GitHub personal access token. Sign in with GitHub instead.
