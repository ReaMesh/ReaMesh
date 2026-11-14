# Translating ReaMesh

The app's text is one JSON file per language in
[`app/src/locales/`](app/src/locales/). English (`en.json`) is the source.
Adding a language means adding one file, no code.

## Adding a language

1. Copy `en.json` to `<code>.json` (`fr`, `es`, `pt-BR`, …).
2. Set `languageName` to the language's own name ("Français").
3. Translate the values. Leave the keys and the `{placeholders}` as they are.
4. Run `cd app && bun install && bun run test` and open a pull request.

A partial translation is fine: a key you leave out is shown in English.

## Plurals

A message with `{count}` can have one variant per plural form of your
language. The plain key is the general form; add `One`, `Two`, `Few`, `Many`
or `Zero` to the key for the others:

```json
"daysLeftOne": "Został {count} dzień",
"daysLeftFew": "Zostały {count} dni",
"daysLeftMany": "Zostało {count} dni",
"daysLeft": "Zostało {count} dnia"
```

English writes its `One` variants with a literal "1". Where your language
uses that form for other numbers too (French for 0, Russian for 21), write
`{count}` instead.

## Wording

- ReaMesh, REAPER and plugin format names are never translated.
- Use one word each for *session*, *host*, *relay* and *invite code*
  throughout.
- The window is narrow: keep buttons and titles close to the English length.
- Take extra care with the join dialog (`trust…`), passwords and blocking
  people. If a message is unclear, ask in the pull request.
