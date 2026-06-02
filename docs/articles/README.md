# Articles

Working materials for blog posts and articles about AnyClaude's
development. Not finished prose — these are raw notes, outlines,
timelines, and quotable excerpts collected while doing the work, so
that an article can later be assembled without forgetting context.

Each subfolder is one article-in-progress. The folder layout aims to
keep the four kinds of material a writer always needs nearby:

- `outline.md` — structure and key points of the planned article.
- `research-notes.md` — substantive findings, written in a style usable
  by the article almost verbatim.
- `timeline.md` — what was done and when, with commit links, so the
  narrative arc is not lost.
- `quotes-and-numbers.md` — exact constants, file:line references, code
  snippets, and standout quotes worth keeping.

When a folder graduates into a published article, the article itself
should live elsewhere (a personal blog, a company blog, dev.to, etc.).
This folder is the scratchpad.

## Published

- **"Why I Wrote My Own Terminal Emulator (and How)"** (2026-06-01) — the
  GPU-terminal article, written from the `warp-gpu-terminal/` materials
  below. Final framing is "why and how I wrote my own emulator" (deep-dive
  walkthroughs: grid model, parser, glyph atlas, glyph placement, pixel
  scroll), not the scroll-first working title in the outline. Lives in the
  personal blog vault (EN + RU). Considered finished; minor fixes may land.

## Materials

- [warp-gpu-terminal/](warp-gpu-terminal/) — research from Warp's
  open-source codebase that informed the `term_gpu` design and the
  Phase 3.5 smooth-scroll prototype. Source material for the article above.
