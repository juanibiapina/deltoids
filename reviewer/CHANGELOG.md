# Changelog

All notable changes to the browser PR reviewer (review.deltoids.dev) are
documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

- The file tree highlights the file you are currently reading and follows along as you scroll, so you always know where you are in the pull request. The highlighted row stays in view within the tree.
- Hunks are separated by a divider showing how many unchanged lines are hidden — above the first hunk, between hunks, and below the last. Click a divider to reveal that code in place; the hunks then merge into one continuous view with no repeated header.
- Pick the syntax highlighting theme from a toolbar selector (grouped into dark and light themes, or "Auto" to follow the page theme). Your choice is remembered, and first-time visitors get Tokyo Night on dark or GitHub on light. Switching recolors the open diff instantly without re-fetching the pull request.

### Changed

- The scrollbars throughout the reviewer are now thin and theme-tinted instead of the chunky default ones.
