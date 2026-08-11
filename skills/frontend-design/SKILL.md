---
name: frontend-design
description: Define a distinctive, platform-native frontend design specification before implementation, including tokens, structure, states, form factor, and interaction intent.
---

# Frontend design

Act as the design lead before code is written. Turn the confirmed product brief
into a compact `design_spec` that another agent can implement without guessing.

## Required output

Record:

- target platform (`ios`, `android`, or `desktop`) and form factor (`iphone`,
  `phone`, `ipad`, `tablet`, or `desktop`); if absent, use the
  host device context and show the inference for confirmation;
- page and navigation structure, including the primary, empty, loading,
  error, success, and permission states;
- named design tokens for color, typography, spacing, radius, elevation,
  controls, icon treatment, motion, safe-area insets, and content width;
- interaction semantics: navigation depth, back behavior, focus/hover,
  gestures, keyboard/mouse input, and reduced-motion behavior;
- the adapter boundary: shared business state plus platform-specific shell,
  navigation, controls, and responsive layout rules.

Prefer real subject-specific content and one memorable visual decision over
generic card grids. Use an image-generation capability only for original
bitmap art that the brief actually needs; normal UI icons remain SVG/Lucide.

## Platform gate

Do not describe a design as merely “responsive”. Apply the platform matrix in
[references/platform-matrix.md](references/platform-matrix.md), then return the
spec to the orchestrator for confirmation before generation.

## Handoff

Keep the result concise and machine-readable where possible. The next stage
may consume the spec as JSON, but it must still be understandable in a user
confirmation prompt. Do not edit source files or install packages in this
skill.
