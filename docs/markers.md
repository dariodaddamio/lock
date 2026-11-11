---
title: Lock markers
nav_order: 6
---

# Lock markers

Your agent writes the markers, so you do not need to learn them. This section is
for reference.

```ts
/* LOCKED: pricing-copy START */
export const tagline = "Pay for what you ship.";
/* LOCKED: pricing-copy END */
```

```md
<!-- LOCKED: release-history START -->
## 1.0.0 - October 30, 2026
<!-- LOCKED: release-history END -->
```

| File types | The markers are written as |
| --- | --- |
| JavaScript, TypeScript, JSX, TSX, CSS, SCSS, Less | `/* */`, `//`, or `{/* */}` in JSX |
| Rust, Go, Java, Kotlin, Swift, C, C++, C#, PHP, Scala, Dart, Zig | `/* */` or `//` |
| Markdown, MDX, HTML, SVG, XML | `<!-- -->` |
| Vue, Svelte, Astro | Any of the above |
| Python, Ruby, shell, PowerShell, YAML, TOML, Terraform, Nix | `#` |
| SQL, Lua, Haskell, Elm | `--` |
| Any other file, such as JSON, an image, or a Dockerfile | A whole-file lock in `.locks/files.json` |

- Each marker is a comment at the start of its own line, and the id uses lowercase
  letters, digits, and hyphens and is unique in its file.
- Text after `START` or `END` is a note for people, and `lock` ignores it.
- A region cannot be inside another region.
- In Markdown, a marker inside a fenced code block is an example, so it does not
  lock anything.
- A broken marker, such as a `START` without an `END`, blocks every agent edit to
  that file until someone repairs it.
- A UTF-8 byte-order mark before a line-1 marker is ignored when reading markers.
- `LOCKED:` in a file type that cannot hold markers locks nothing. `lock check`,
  `lock doctor`, and the agent hook warn about those files.

To lock a file that cannot hold a comment, add its path to `.locks/files.json`:

```json
{ "files": ["public/catalog.json"] }
```
