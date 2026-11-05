# MD3 Basics (Material Design 3)

MD3 is Google's design system — also called **Material You**. It is not a framework or a language: it is a set of rules for colour, type, shape, and components that you implement in CSS, Android (Jetpack Compose), Flutter, or on the web with Material Web components.

Its big idea over MD2: **personal, adaptive, expressive**. Colour can be generated from one seed colour (or the user's wallpaper — "dynamic colour"), layouts adapt to screen size, and shapes are rounder.

## 1. Colour — roles, not hex codes

You never scatter raw hex values. One **seed colour** generates tonal palettes, which map to named **roles**. Components only use roles, so light/dark themes and rebranding come free.

| Role | Used for |
|---|---|
| `primary` / `on-primary` | Main actions (filled button bg / its text) |
| `primary-container` / `on-primary-container` | Softer highlight, selected states |
| `secondary`, `tertiary` (+ their containers) | Supporting accents, variety |
| `surface`, `surface-variant` | Backgrounds, cards |
| `on-surface`, `on-surface-variant` | Text on surfaces (strong / muted) |
| `outline` | Borders, dividers |
| `error` / `on-error` | Error states |

Every role has a light and a dark value. In CSS you implement roles as variables and flip them in dark mode. Contrast between a role and its `on-*` pair is guaranteed by the system — that is the point.

## 2. Typography — a fixed type scale

15 named styles in 5 groups, each with size/weight/line-height already decided. Use the name, not a font size.

```text
display  L / M / S    biggest, short headlines only
headline L / M / S    section headings
title    L / M / S    card and dialog titles
body     L / M / S    paragraphs (body-medium is the default)
label    L / M / S    buttons, chips, captions
```

Default typeface is Roboto (Roboto Flex on newer Android). One scale everywhere is what makes unrelated screens feel like one product.

## 3. Shape — corner radius scale

```text
none 0dp   extra-small 4dp   small 8dp   medium 12dp
large 16dp   extra-large 28dp   full (pill / circle)
```

Cards are medium, dialogs extra-large, buttons and chips full. MD3 is noticeably rounder than MD2.

## 4. Elevation — mostly tone, not shadow

Five levels (0–5). Higher surfaces get a lighter surface tint plus, only when needed, a soft shadow. A level-1 card on a level-0 background usually needs no shadow at all.

## 5. Components — same pieces, everywhere

Build screens from the standard set so users already know how they behave:

- **Buttons:** filled (one main action per screen), filled-tonal, outlined, text, elevated.
- **FAB** (floating action button) for the single most common action; **chips** for filters/choices; **cards** for grouped content.
- **Navigation:** bottom navigation bar (phones) → navigation rail (tablets) → navigation drawer (desktop). Same destinations, container adapts.
- **Controls:** switch (on/off — preferred over checkbox for settings), checkbox, radio, slider, segmented buttons.
- **Feedback:** dialogs, snackbar, progress indicators, badges.

Key rules: state layers (a translucent overlay shows hover/press/drag on interactive elements), touch targets at least 48dp, and disabled states are the same component at reduced opacity — never a different style.

## 6. Layout — window size classes

Design for width classes, not devices: **compact** (<600dp, phone), **medium** (600–840dp), **expanded** (>840dp). Use 8dp spacing rhythm, and let navigation and columns change per class instead of shrinking a desktop layout.

## 7. Motion

Short, purposeful transitions: standard easing, roughly 100–300ms for most component transitions, shared-element/container transforms for moving between screens. Nothing bounces forever; respect reduced-motion settings.

See [demo.html](demo.html): one self-contained page showing colour roles (with a light/dark toggle), the type scale, shape scale, and the main components in plain CSS variables — open it in a browser, no build step.
